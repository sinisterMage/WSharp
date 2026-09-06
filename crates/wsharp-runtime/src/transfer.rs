//! Copying an object graph from one worker's heap into another's.
//!
//! No object is reachable from two workers and nothing is sent by pointer, so
//! sending a value means copying it. The copy crosses the boundary as **plain
//! bytes** rather than as a graph of pointers, and that is the whole design:
//!
//! * The sender's heap is the only one it may touch. Allocating into another
//!   worker's heap would need its lock, its allocation buffer and its mark
//!   parity -- and the object would be judged by a collector that had never
//!   seen it born.
//! * A wire buffer is ordinary memory. It can sit in a queue for as long as it
//!   likes without any collector having an opinion about it, which is what
//!   lets a message outlive the call that sent it.
//!
//! What the walk needs, it already had: [`types::for_each_ptr_offset`] is the
//! single definition of where an object's references are, and it covers an
//! array's elements as well as a struct's fields.
//!
//! Decoding is the part that needed something new. It builds a graph in Rust
//! locals, which no stack map describes -- so every object it makes is pinned
//! on the runtime root list until the whole graph is built. See
//! [`crate::worker::Pinned`].

use std::collections::HashMap;

use crate::header::{HEADER_SIZE, TypeId, align_up, type_id_of};
use crate::heap::ws_alloc;
use crate::types;
use crate::worker::Pinned;

/// One object, as bytes.
///
/// The body only: the header is rebuilt by the receiving allocator, because a
/// header is about the heap the object is in -- its mark parity, its reference
/// count, its flags -- and none of that travels.
struct Object {
    type_id: TypeId,
    /// The element count of a string or an array; zero for anything else.
    aux: u64,
    /// The body, with every pointer slot holding an *index* into the message
    /// plus one rather than an address. Zero means null.
    body: Vec<u8>,
}

/// A graph of objects, flattened.
pub struct Wire {
    objects: Vec<Object>,
}

impl Wire {
    pub fn objects(&self) -> usize {
        self.objects.len()
    }

    pub fn bytes(&self) -> usize {
        self.objects.iter().map(|o| o.body.len()).sum()
    }
}

/// Copy the graph reachable from `root` out of this worker's heap.
///
/// Breadth-first with a table of what has already been seen, so a cycle
/// terminates and something referenced twice is copied once -- which is what
/// makes the graph on the other side the same *shape*, not merely the same
/// contents.
///
/// # Safety
/// `root` must be null or a live object with a registered type, and this must
/// run on the thread that owns the heap it is in.
pub unsafe fn encode(root: *mut u8) -> Wire {
    let mut wire = Wire {
        objects: Vec::new(),
    };
    let mut seen: HashMap<usize, u32> = HashMap::new();
    let mut queue: Vec<*mut u8> = Vec::new();

    let intern = |p: *mut u8, seen: &mut HashMap<usize, u32>, queue: &mut Vec<*mut u8>| -> u64 {
        if p.is_null() {
            return 0;
        }
        // While a trace is moving objects a field may name one that has gone,
        // so ask where it lives now -- the same thing the write barrier does,
        // and for the same reason.
        let p = if crate::gc::evacuating() {
            unsafe { crate::evacuate::ws_resolve(p) }
        } else {
            p
        };
        let next = seen.len() as u32;
        match seen.entry(p as usize) {
            std::collections::hash_map::Entry::Occupied(e) => *e.get() as u64 + 1,
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(next);
                queue.push(p);
                next as u64 + 1
            }
        }
    };

    intern(root, &mut seen, &mut queue);
    let mut at = 0;
    while at < queue.len() {
        let obj = queue[at];
        at += 1;
        let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
            // Not something with a layout: nothing can be said about its
            // shape, so it travels as an empty object rather than as garbage.
            wire.objects.push(Object {
                type_id: unsafe { type_id_of(obj) },
                aux: 0,
                body: Vec::new(),
            });
            continue;
        };
        let size = unsafe { types::object_size(obj) }.unwrap_or(HEADER_SIZE);
        let body_len = size.saturating_sub(HEADER_SIZE) as usize;
        let mut body = vec![0u8; body_len];
        unsafe {
            std::ptr::copy_nonoverlapping(obj.add(HEADER_SIZE as usize), body.as_mut_ptr(), body_len)
        };

        // Every reference becomes an index. Collected first so that `intern`
        // can borrow the queue while `for_each_ptr_offset` borrows nothing.
        let mut slots: Vec<u32> = Vec::new();
        unsafe { types::for_each_ptr_offset(obj, info, |offset| slots.push(offset)) };
        for offset in slots {
            let at = (offset - HEADER_SIZE) as usize;
            let field = unsafe { (obj.add(offset as usize) as *const *mut u8).read() };
            let index = intern(field, &mut seen, &mut queue);
            body[at..at + 8].copy_from_slice(&index.to_le_bytes());
        }

        let aux = if info.has_variable_size() {
            unsafe { (obj.offset(crate::header::AUX_OFFSET as isize) as *const u64).read() }
        } else {
            0
        };
        wire.objects.push(Object {
            type_id: unsafe { type_id_of(obj) },
            aux,
            body,
        });
    }
    wire
}

/// Build the graph again in *this* worker's heap.
///
/// Two passes, and the split is load-bearing. The first allocates every object
/// with its pointer slots left null, because an allocation is a safepoint and a
/// collection that ran between two of them would otherwise read a slot holding
/// an index rather than an address. The second writes the pointers, and
/// allocates nothing at all -- so nothing can move underneath it.
///
/// Every object is pinned as it is made, and stays pinned until the graph is
/// finished: the half-built pieces live in a `Vec` here, which no stack map
/// describes.
///
/// # Safety
/// `wire` must have come from [`encode`], and every type id in it must be
/// registered in this process.
pub unsafe fn decode(wire: &Wire) -> *mut u8 {
    if wire.objects.is_empty() {
        return std::ptr::null_mut();
    }
    let pinned = Pinned::new();
    let mut made: Vec<*mut u8> = Vec::with_capacity(wire.objects.len());

    for object in &wire.objects {
        let size = align_up((HEADER_SIZE + object.body.len() as u32).max(HEADER_SIZE));
        let obj = ws_alloc(object.type_id, size as u64, object.aux);
        // Pinned before anything else can allocate: the next iteration of this
        // loop is a safepoint, and this object is in no stack map.
        pinned.add(obj);
        made.push(obj);

        unsafe {
            std::ptr::copy_nonoverlapping(
                object.body.as_ptr(),
                obj.add(HEADER_SIZE as usize),
                object.body.len(),
            )
        };
        // The indices are not addresses. Zeroed here, with no allocation in
        // between, so no collector ever reads one as a reference.
        if let Some(info) = types::info(object.type_id) {
            unsafe {
                types::for_each_ptr_offset(obj, info, |offset| {
                    (obj.add(offset as usize) as *mut u64).write(0)
                })
            };
        }
    }

    // No allocation from here, so no safepoint: the addresses in `made` stay
    // good for the whole pass.
    for (object, &obj) in wire.objects.iter().zip(&made) {
        let Some(info) = types::info(object.type_id) else {
            continue;
        };
        let mut slots: Vec<u32> = Vec::new();
        unsafe { types::for_each_ptr_offset(obj, info, |offset| slots.push(offset)) };
        if slots.is_empty() {
            continue;
        }
        // Through the write barrier, exactly as generated code's stores are.
        // An initialising store is no exception: `ws_alloc` is a safepoint, and
        // a collection there clears the logged bit before the fields are
        // written.
        unsafe { crate::gc::ws_log_object(obj) };
        for offset in slots {
            let at = (offset - HEADER_SIZE) as usize;
            let mut index = [0u8; 8];
            index.copy_from_slice(&object.body[at..at + 8]);
            let index = u64::from_le_bytes(index);
            let target = match index {
                0 => std::ptr::null_mut(),
                n => made[(n - 1) as usize],
            };
            unsafe { (obj.add(offset as usize) as *mut *mut u8).write(target) };
        }
    }

    let root = made[0];
    drop(pinned);
    root
}

/// Copy an array out of this heap and build it again in the same one.
///
/// The round trip a message makes, with both halves on one worker -- which is
/// the whole of the copy except for which heap the second half runs on. A
/// builtin so that the walk can be asserted on from W#, where `--gc-stress`
/// turns every allocation `decode` makes into a collection.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `a` must be null or a
/// live array.
pub unsafe extern "C" fn ws_transfer_roundtrip(a: *mut u8) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    // Encoded before anything is allocated: `a` is a Rust local that no stack
    // map describes, and `decode` allocates.
    let wire = unsafe { encode(a) };
    unsafe { decode(&wire) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::TYPE_ID_FIRST_USER;
    use crate::types::TypeLayout;

    fn registered(id: TypeId, size: u32, ptrs: Vec<u32>) -> TypeId {
        crate::types::register_type(id, TypeLayout::fixed(format!("transfer {id}"), size, ptrs));
        crate::types::publish();
        id
    }

    /// A cycle survives the round trip as a cycle, and shared structure stays
    /// shared -- which is what "the same graph" has to mean.
    #[test]
    fn a_cycle_is_copied_once() {
        let _serial = crate::test_support::SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Two pointer fields at 16 and 24.
        let id = registered(TYPE_ID_FIRST_USER + 40, 32, vec![16, 24]);

        let a = ws_alloc(id, 32, 0);
        let b = ws_alloc(id, 32, 0);
        unsafe {
            (a.add(16) as *mut *mut u8).write(b);
            (b.add(16) as *mut *mut u8).write(a);
            // Both point at `b`, so the copy must not make two of it.
            (a.add(24) as *mut *mut u8).write(b);
        }

        let wire = unsafe { encode(a) };
        assert_eq!(wire.objects(), 2, "a cycle is two objects, not for ever");

        let copy = unsafe { decode(&wire) };
        assert_ne!(copy, a);
        unsafe {
            let copy_b = (copy.add(16) as *const *mut u8).read();
            assert_ne!(copy_b, b, "the copy is in this heap, not the original's");
            assert_eq!((copy_b.add(16) as *const *mut u8).read(), copy, "cycle kept");
            assert_eq!(
                (copy.add(24) as *const *mut u8).read(),
                copy_b,
                "sharing kept"
            );
        }
    }

    #[test]
    fn null_and_nothing_round_trip() {
        let _serial = crate::test_support::SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let wire = unsafe { encode(std::ptr::null_mut()) };
        assert_eq!(wire.objects(), 0);
        assert!(unsafe { decode(&wire) }.is_null());
    }
}
