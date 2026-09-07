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
pub mod fs;
pub mod gc;
pub mod header;
pub mod heap;
pub mod io;
pub mod mark;
pub mod net;
pub mod os;
pub mod rpc;
pub mod stackwalk;
pub mod strings;
pub(crate) mod sys;
pub mod transfer;
pub mod types;
pub mod worker;

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
