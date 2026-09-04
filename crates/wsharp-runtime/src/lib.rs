//! The W# runtime: the heap, the object header, the type registry, the
//! collector and the builtin functions that JIT-compiled code calls into.
//!
//! This is a leaf crate -- it depends on nothing else in the workspace, so both
//! the type checker and the code generator can use it.

pub mod builtins;
pub mod evacuate;
pub mod gc;
pub mod header;
pub mod heap;
pub mod io;
pub mod mark;
pub mod stackwalk;
pub mod strings;
pub mod types;

pub use builtins::{Builtin, BuiltinTy, builtins, runtime_symbols};
pub use header::{HEADER_SIZE, TypeId};
pub use heap::{HeapStats, heap_stats, in_heap, ws_alloc};
pub use types::{TypeInfo, TypeLayout, info, layout_of, publish, register_type};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    /// Taken by every unit test that touches process-wide collector state --
    /// the mark parity, the stress flag -- because the test binary runs its
    /// tests in parallel and they all share one heap.
    pub static SERIAL: Mutex<()> = Mutex::new(());
}
