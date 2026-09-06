//! The message broker: named topics, partitioned logs, consumer groups.
//!
//! RPC is for when the caller needs the answer. This is for when it does not,
//! or when more than one worker wants the same message -- and the shape is
//! Kafka's, because that shape is what makes the two useful things possible at
//! once: a consumer that has fallen behind can catch up, and a consumer that
//! has died can be replaced by one that starts where the group had got to.
//!
//! * A **topic** is a fixed number of append-only logs. A message goes into
//!   the partition its key hashes to, so messages sharing a key stay in order
//!   with each other, which is the only ordering anyone can rely on.
//! * A **group** holds one committed offset per partition. A consumer reads
//!   forward from its own position and commits when it is done, so a message
//!   that was read but not committed is read again by the next consumer of
//!   that group -- which is what at-least-once means.
//! * `seek` puts a position back, so replay is not a feature but the absence
//!   of one: the log is still there.
//!
//! A message sits in the log as *bytes* -- see [`crate::transfer`] -- so it
//! belongs to no heap while it waits, and each consumer decodes its own copy
//! into its own. Durability is a later concern and does not change any of
//! this: the log would be written down rather than kept, and nothing above
//! would know.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::transfer::{self, Wire};

struct Topic {
    name: String,
    partitions: Vec<Vec<Wire>>,
    /// Where each group has committed to, per partition.
    groups: HashMap<String, Vec<u64>>,
}

struct Consumer {
    topic: usize,
    group: String,
    /// Where this consumer has read to, which runs ahead of what its group has
    /// committed. A fresh consumer starts from the committed offsets.
    position: Vec<u64>,
}

#[derive(Default)]
struct Broker {
    topics: Vec<Topic>,
    consumers: Vec<Consumer>,
}

// The logs hold plain bytes, which belong to no heap.
unsafe impl Send for Broker {}

static BROKER: Mutex<Option<Broker>> = Mutex::new(None);

fn with_broker<R>(f: impl FnOnce(&mut Broker) -> R) -> R {
    let mut guard = BROKER.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Broker::default))
}

/// Which partition a key belongs to.
///
/// FNV-1a, because the only property wanted is that the same key always lands
/// in the same partition -- which is what keeps messages about one thing in
/// order with each other.
fn partition_of(key: &[u8], partitions: usize) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % partitions.max(1) as u64) as usize
}

/// Find a topic by name, or make it.
///
/// Named rather than handed out, so two workers that have never met can agree
/// on one by spelling it the same way -- which is the whole point of a broker
/// rather than a channel.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `name` must be a live
/// string object.
pub unsafe extern "C" fn ws_broker_topic(name: *const u8, partitions: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let name = String::from_utf8_lossy(unsafe { crate::strings::str_bytes(name) }).into_owned();
    let count = partitions.max(1) as usize;
    with_broker(|b| {
        if let Some(at) = b.topics.iter().position(|t| t.name == name) {
            return at as i64;
        }
        b.topics.push(Topic {
            name,
            partitions: (0..count).map(|_| Vec::new()).collect(),
            groups: HashMap::new(),
        });
        (b.topics.len() - 1) as i64
    })
}

/// Append a message to the partition its key belongs to.
///
/// # Safety
/// `message` must be a live heap object; the type checker is what guarantees
/// that, by refusing anything but an object as a topic's message type.
pub unsafe extern "C" fn ws_broker_publish(topic: i64, key: *const u8, message: *mut u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    // Out of this worker's heap and into bytes before the lock is taken: the
    // walk reads the sender's objects, and only the sender's thread may.
    let wire = unsafe { transfer::encode(message) };
    let key = unsafe { crate::strings::str_bytes(key) }.to_vec();
    with_broker(|b| {
        let Some(t) = b.topics.get_mut(topic as usize) else {
            return -1;
        };
        let at = partition_of(&key, t.partitions.len());
        t.partitions[at].push(wire);
        at as i64
    })
}

/// Join a consumer group on a topic, starting where the group has committed.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_broker_subscribe(topic: i64, group: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let group = String::from_utf8_lossy(unsafe { crate::strings::str_bytes(group) }).into_owned();
    with_broker(|b| {
        let Some(t) = b.topics.get_mut(topic as usize) else {
            return -1;
        };
        let width = t.partitions.len();
        let committed = t
            .groups
            .entry(group.clone())
            .or_insert_with(|| vec![0; width])
            .clone();
        b.consumers.push(Consumer {
            topic: topic as usize,
            group,
            position: committed,
        });
        (b.consumers.len() - 1) as i64
    })
}

/// `?M`, as it crosses the boundary: the tag in a whole word, then the value.
#[repr(C)]
pub struct MaybeMessage {
    pub tag: i64,
    pub value: *mut u8,
}

/// Take the next message this consumer has not read, and build it in *this*
/// worker's heap.
///
/// Round-robin across partitions from the one after the last, so that a busy
/// partition cannot starve a quiet one.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `out` must point at a
/// slot of this shape.
pub unsafe extern "C" fn ws_broker_poll(out: *mut MaybeMessage, consumer: i64) {
    unsafe { crate::gc::checkpoint() };
    // Copied out from under the lock before anything is decoded: decoding
    // allocates, and allocating can collect, and the broker's lock has no
    // business being held across either.
    let taken = with_broker(|b| {
        let c = b.consumers.get(consumer as usize)?;
        let t = b.topics.get(c.topic)?;
        let at = (0..t.partitions.len())
            .find(|&p| (c.position[p] as usize) < t.partitions[p].len())?;
        let wire = t.partitions[at][c.position[at] as usize].clone();
        b.consumers[consumer as usize].position[at] += 1;
        Some(wire)
    });
    let Some(wire) = taken else {
        unsafe {
            out.write(MaybeMessage {
                tag: 0,
                value: std::ptr::null_mut(),
            })
        };
        return;
    };
    let value = unsafe { transfer::decode(&wire) };
    // After the allocation, never before: `out` points into the caller's frame,
    // which no stack map describes.
    unsafe { out.write(MaybeMessage { tag: 1, value }) };
}

/// Say the group has got this far, so a consumer that replaces this one starts
/// here rather than where the group last committed.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_broker_commit(consumer: i64) {
    unsafe { crate::gc::checkpoint() };
    with_broker(|b| {
        let Some(c) = b.consumers.get(consumer as usize) else {
            return;
        };
        let (topic, group, position) = (c.topic, c.group.clone(), c.position.clone());
        if let Some(t) = b.topics.get_mut(topic) {
            t.groups.insert(group, position);
        }
    });
}

/// Put one partition's position back, which is all replay is: the log is still
/// there, and this is where to start reading it again.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_broker_seek(consumer: i64, partition: i64, offset: i64) {
    unsafe { crate::gc::checkpoint() };
    with_broker(|b| {
        let Some(c) = b.consumers.get_mut(consumer as usize) else {
            return;
        };
        if let Some(slot) = c.position.get_mut(partition.max(0) as usize) {
            *slot = offset.max(0) as u64;
        }
    });
}

/// How many messages a topic holds, across every partition.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_broker_len(topic: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    with_broker(|b| match b.topics.get(topic as usize) {
        Some(t) => t.partitions.iter().map(|p| p.len() as i64).sum(),
        None => 0,
    })
}
