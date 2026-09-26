//! The W# runtime: the heap, the object header, the type registry, the
//! collector and the builtin functions that JIT-compiled code calls into.
//!
//! This is a leaf crate -- it depends on nothing else in the workspace, so both
//! the type checker and the code generator can use it.

pub mod aot;
pub mod broker;
pub mod builtins;
pub mod bytes;
pub mod crypto;
pub mod evacuate;
pub mod ffi;
pub mod fs;
pub mod gc;
pub mod header;
pub mod heap;
pub mod io;
pub mod mark;
pub mod net;
pub mod os;
pub mod process;
pub mod rpc;
pub mod signals;
pub mod stackwalk;
pub mod strings;
pub(crate) mod sys;
pub mod time;
pub mod transfer;
pub mod types;
pub mod worker;

/// How deep a struct `==` may recurse before it refuses.
///
/// `==` on a struct compares its fields and a struct field recurses, so a
/// value that reaches itself -- a parent pointer, a doubly linked list, a
/// graph node -- would recurse without end. It used to: the process ran its
/// stack out and died on a signal, which is a crash with no W# diagnostic.
/// The generated comparison counts its depth against this instead, and
/// `PANIC_EQ_TOO_DEEP` reports it.
///
/// A named bound with no knob, in the shape of `std/json`'s `MAX_DEPTH` and
/// `std/x509`'s `MAX_CHAIN`. The number has to clear two bars at once, and
/// they pull opposite ways:
///
/// * **Below the smallest stack.** A worker thread gets Rust's default 2 MiB,
///   and one level of nesting costs two generated frames -- the dispatching
///   entry point and the exact comparison. At 2048 levels that is well under
///   a quarter of that stack, so the bound is reached before the stack is.
/// * **Above anything that used to work.** A comparison deeper than this
///   already aborted, so nothing that previously answered now refuses.
///
/// Defined here rather than in the code generator because the generated check
/// and the message that explains it must name one number; two would drift,
/// and the drift would be a message stating a bound that is not the one
/// enforced.
pub const EQ_MAX_DEPTH: i64 = 2048;

pub use builtins::{Builtin, BuiltinTy, builtins, runtime_symbols};
pub use header::{HEADER_SIZE, TypeId};
pub use heap::{HeapStats, heap_stats, in_heap, ws_alloc};
pub use types::{TypeInfo, TypeLayout, info, layout_of, publish, register_type};
pub use worker::Worker;

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    /// Taken by every unit test that touches process-wide collector state.
    ///
    /// That is now only the stress flag and the type registry: a test thread
    /// is a worker of its own, so its heap, its buffers and its mark parity
    /// are its own too, and the tests that used to contend over those no
    /// longer can.
    pub static SERIAL: Mutex<()> = Mutex::new(());
}
