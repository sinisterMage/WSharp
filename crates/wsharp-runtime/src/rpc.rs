//! Workers, and the typed calls between them.
//!
//! A worker is an OS thread that owns its heap, running one module's service.
//! A caller holds a handle -- an index, not a pointer, because a worker's
//! objects are not the caller's to hold -- and every call copies its arguments
//! there and its result back. A call returns `!T` because a worker can die,
//! and that is not an exceptional case worth a second mechanism.
//!
//! The marshalling meets generated code exactly twice, and both times through
//! a buffer of machine words:
//!
//! * the **call site** writes its arguments into one and reads its result from
//!   another, and
//! * a **trampoline** -- one generated function per method -- reads the
//!   arguments out of a buffer and calls the real function.
//!
//! Both are generated code, which is the point: a reference that moves from
//! one object into another goes through the write barrier, the load barrier
//! and the stack maps by construction there, and a hand-written Rust caller
//! would have none of the three. What is left for this file is bytes, which is
//! what it may touch.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Condvar, Mutex};

use crate::builtins::{ERROR_SPAWN_FAILED, ERROR_WORKER_DIED};
use crate::transfer::{self, Wire};
use crate::worker::{Pinned, Worker};

/// What one machine word of an argument or a result is.
///
/// Only two kinds, because only one question matters: whether the word is a
/// reference, and so has to be copied rather than moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Scalar,
    Ref,
}

/// One service, as the code generator describes it.
///
/// `init` and `call` are trampolines: generated functions with a fixed C
/// signature that unpack a word buffer and call the real one.
pub struct ServiceCode {
    pub name: &'static str,
    /// `extern "C" fn(argv: *const u64, out: *mut u64)`
    pub init: *const u8,
    pub init_args: &'static [SlotKind],
    pub methods: &'static [MethodCode],
}

pub struct MethodCode {
    pub name: &'static str,
    /// `extern "C" fn(state: *mut u8, argv: *const u64, out: *mut u64)`
    pub call: *const u8,
    pub args: &'static [SlotKind],
    pub ret: &'static [SlotKind],
}

// The pointers are into JIT-compiled code, which lives as long as the process.
unsafe impl Send for ServiceCode {}
unsafe impl Sync for ServiceCode {}
unsafe impl Send for MethodCode {}
unsafe impl Sync for MethodCode {}

type InitFn = unsafe extern "C" fn(*const u64, *mut u64);
type CallFn = unsafe extern "C" fn(*mut u8, *const u64, *mut u64);

static SERVICES: Mutex<Vec<&'static ServiceCode>> = Mutex::new(Vec::new());

/// Publish the services a program declares. Called once, before any code runs.
pub fn register_services(services: Vec<&'static ServiceCode>) {
    *SERVICES.lock().unwrap_or_else(|e| e.into_inner()) = services;
}

fn service(id: u32) -> Option<&'static ServiceCode> {
    SERVICES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id as usize)
        .copied()
}

/// One word of an argument or a result, on its way between heaps.
///
/// A scalar is the word itself. A reference is the whole graph reachable from
/// it, flattened -- see [`crate::transfer`].
enum Word {
    Scalar(u64),
    Ref(Wire),
}

enum Message {
    Call {
        method: u32,
        args: Vec<Word>,
        reply: Sender<Result<Vec<Word>, i64>>,
    },
    Stop,
}

/// A worker, as the *caller* sees it: a queue and a way to wait for it.
struct Handle {
    /// The queue, and whether the worker is still there to serve it. One lock
    /// covers both, which is what closes the race a caller would otherwise
    /// have with a worker shutting down: a call that got in before the worker
    /// stopped is answered by the drain below, and one that arrives after sees
    /// `alive` false under the same lock and is told so at once. Without that,
    /// a call to a worker that has gone would wait for a reply nobody is left
    /// to send.
    queue: Mutex<Queue>,
    arrived: Condvar,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

struct Queue {
    messages: VecDeque<Message>,
    alive: bool,
}

static HANDLES: Mutex<Vec<&'static Handle>> = Mutex::new(Vec::new());

fn handle(id: i64) -> Option<&'static Handle> {
    if id < 0 {
        return None;
    }
    HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id as usize)
        .copied()
}

/// Pack the words a call site laid out, out of *this* worker's heap.
///
/// # Safety
/// `argv` must point at `kinds.len()` words, and every `Ref` word must hold a
/// live reference.
unsafe fn pack(argv: *const u64, kinds: &[SlotKind]) -> Vec<Word> {
    let mut out = Vec::with_capacity(kinds.len());
    for (i, kind) in kinds.iter().enumerate() {
        let word = unsafe { argv.add(i).read() };
        out.push(match kind {
            SlotKind::Scalar => Word::Scalar(word),
            SlotKind::Ref => Word::Ref(unsafe { transfer::encode(word as *mut u8) }),
        });
    }
    out
}

/// Build the words again in *this* worker's heap, pinning every reference for
/// as long as `pinned` lives.
///
/// # Safety
/// The words must have come from [`pack`] against the same slot kinds.
unsafe fn unpack(words: &[Word], pinned: &Pinned) -> Vec<u64> {
    words
        .iter()
        .map(|w| match w {
            Word::Scalar(v) => *v,
            Word::Ref(wire) => {
                let obj = unsafe { transfer::decode(wire) };
                // The caller holds these in a `Vec` that no stack map
                // describes, and is about to allocate again for the next one.
                pinned.add(obj);
                obj as u64
            }
        })
        .collect()
}

/// Start a worker running `service`, with `init` as its first act.
///
/// Returns the handle, or -1 if the thread could not be started.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `argv` must match the
/// service's `init_args`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_spawn(service_id: u32, argv: *const u64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let Some(code) = service(service_id) else {
        return -1;
    };
    // Packed before the thread exists: the arguments are this worker's, and
    // this is the only thread that may read them.
    let args = unsafe { pack(argv, code.init_args) };

    let handle: &'static Handle = Box::leak(Box::new(Handle {
        queue: Mutex::new(Queue {
            messages: VecDeque::new(),
            alive: true,
        }),
        arrived: Condvar::new(),
        thread: Mutex::new(None),
    }));
    let id = {
        let mut list = HANDLES.lock().unwrap_or_else(|e| e.into_inner());
        list.push(handle);
        (list.len() - 1) as i64
    };

    let spawned = std::thread::Builder::new()
        .name(format!("wsharp-worker-{id}"))
        .spawn(move || worker_main(code, args, handle));
    match spawned {
        Ok(thread) => {
            *handle.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
            id
        }
        Err(_) => -1,
    }
}

/// The tag `@spawn` reports when a thread could not be started.
pub const SPAWN_FAILED_TAG: i64 = ERROR_SPAWN_FAILED;

/// The body of a worker: make the state, then serve until told to stop.
fn worker_main(code: &'static ServiceCode, args: Vec<Word>, handle: &'static Handle) {
    // A worker of its own, and so a heap of its own: this is the whole point.
    // Created by asking, which also installs it for every allocation below.
    let _worker = Worker::current();

    // The state lives as long as the worker does, and nothing on the stack
    // holds it between calls -- so it is pinned, on the same list `decode`
    // uses and for the same reason.
    let state_pin = Pinned::new();
    let mut state_out = [0u64; 1];
    {
        let arg_pin = Pinned::new();
        let argv = unsafe { unpack(&args, &arg_pin) };
        let init: InitFn = unsafe { std::mem::transmute(code.init) };
        unsafe { init(argv.as_ptr(), state_out.as_mut_ptr()) };
    }
    let state = state_out[0] as *mut u8;
    state_pin.add(state);
    drop(args);

    loop {
        let message = {
            let mut queue = handle.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(m) = queue.messages.pop_front() {
                    break m;
                }
                queue = handle
                    .arrived
                    .wait(queue)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        match message {
            Message::Stop => {
                // Under the lock, so that a call either got in before this and
                // is answered below, or arrives after and is refused at once.
                let mut queue = handle.queue.lock().unwrap_or_else(|e| e.into_inner());
                queue.alive = false;
                for pending in queue.messages.drain(..) {
                    if let Message::Call { reply, .. } = pending {
                        let _ = reply.send(Err(ERROR_WORKER_DIED));
                    }
                }
                break;
            }
            Message::Call {
                method,
                args,
                reply,
            } => {
                let Some(m) = code.methods.get(method as usize) else {
                    let _ = reply.send(Err(ERROR_WORKER_DIED));
                    continue;
                };
                let out = {
                    let pin = Pinned::new();
                    let argv = unsafe { unpack(&args, &pin) };
                    let mut out = vec![0u64; m.ret.len()];
                    let call: CallFn = unsafe { std::mem::transmute(m.call) };
                    unsafe { call(state, argv.as_ptr(), out.as_mut_ptr()) };
                    // Packed while still pinned and before anything else can
                    // allocate: a result is a reference into this heap until
                    // it is bytes.
                    unsafe { pack(out.as_ptr(), m.ret) }
                };
                let _ = reply.send(Ok(out));
            }
        }
    }

    // The heap goes when the process does; what matters is that the collector
    // is not left half way through anything -- and that what this thread
    // allocated is on the statistics, which are otherwise still sitting in its
    // own allocation buffer where only it can publish them.
    crate::heap::flush_local_counters();
    crate::mark::quiesce(Worker::current());
}

/// Call a method on a worker, and wait for the answer.
///
/// Returns 0 and fills `out` on success; otherwise the error's tag, which is
/// `WorkerDied` whatever went wrong -- a worker that cannot answer is a worker
/// that has gone, from the caller's side of the boundary.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `argv` and `out` must
/// match the method's slot kinds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_rpc_call(
    worker: i64,
    service_id: u32,
    method: u32,
    argv: *const u64,
    out: *mut u64,
) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let (Some(code), Some(handle)) = (service(service_id), handle(worker)) else {
        return ERROR_WORKER_DIED;
    };
    let Some(m) = code.methods.get(method as usize) else {
        return ERROR_WORKER_DIED;
    };

    // Packed before anything is sent, out of this worker's heap and on its own
    // thread -- the only one that may read it.
    let args = unsafe { pack(argv, m.args) };
    let (tx, rx): (Sender<_>, Receiver<_>) = channel();
    {
        let mut queue = handle.queue.lock().unwrap_or_else(|e| e.into_inner());
        if !queue.alive {
            return ERROR_WORKER_DIED;
        }
        queue.messages.push_back(Message::Call {
            method,
            args,
            reply: tx,
        });
    }
    handle.arrived.notify_all();

    let Ok(reply) = rx.recv() else {
        return ERROR_WORKER_DIED;
    };
    let words = match reply {
        Ok(words) => words,
        Err(tag) => return tag,
    };

    // Decoded into *this* heap, and written into the caller's slot only
    // afterwards: `out` points into a frame no stack map describes, so a
    // reference parked there would be invisible to a collection this decode
    // triggers.
    let pin = Pinned::new();
    let slots = unsafe { unpack(&words, &pin) };
    for (i, word) in slots.iter().enumerate() {
        unsafe { out.add(i).write(*word) };
    }
    0
}

/// Wait for a worker to finish what it is doing, and shut it down.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_join(worker: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let Some(handle) = handle(worker) else {
        return ERROR_WORKER_DIED;
    };
    {
        let mut queue = handle.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.messages.push_back(Message::Stop);
    }
    handle.arrived.notify_all();
    let thread = handle
        .thread
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    match thread {
        // Already joined, or never started.
        None => ERROR_WORKER_DIED,
        Some(t) => match t.join() {
            Ok(()) => 0,
            Err(_) => ERROR_WORKER_DIED,
        },
    }
}

/// Stop every worker still running, at exit.
///
/// A worker parked on its queue would otherwise keep the process alive, and a
/// worker in the middle of a trace would be left half way through it.
pub fn stop_all() {
    let handles: Vec<&'static Handle> = HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .collect();
    for handle in handles {
        {
            let mut queue = handle.queue.lock().unwrap_or_else(|e| e.into_inner());
            queue.messages.push_back(Message::Stop);
        }
        handle.arrived.notify_all();
        let thread = handle
            .thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(t) = thread {
            let _ = t.join();
        }
    }
}
