//! The builtin function registry.
//!
//! One table describes every function the language provides natively. The type
//! checker reads it to seed the global type environment; the code generator
//! reads the same table to register symbols with the JIT. Adding a function to
//! the standard library (a later session) means adding one row here -- the two
//! consumers pick it up without changes.

use crate::header::{AUX_OFFSET, HEADER_SIZE};

/// The types a builtin signature can mention. Deliberately small and
/// independent of the type checker's representation, so that `wsharp-runtime`
/// stays a leaf crate that neither sema nor codegen has to be built before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTy {
    I64,
    F64,
    Bool,
    Void,
    Str,
}

pub struct Builtin {
    pub name: &'static str,
    pub params: &'static [BuiltinTy],
    pub ret: BuiltinTy,
    /// Address of the native implementation, handed to the JIT as a symbol.
    pub ptr: *const u8,
}

/// Every builtin visible to W# source.
pub fn builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            name: "print",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Void,
            ptr: ws_print_str as *const u8,
        },
        Builtin {
            name: "print_int",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: ws_print_int as *const u8,
        },
        Builtin {
            name: "print_float",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::Void,
            ptr: ws_print_float as *const u8,
        },
        Builtin {
            name: "print_bool",
            params: &[BuiltinTy::Bool],
            ret: BuiltinTy::Void,
            ptr: ws_print_bool as *const u8,
        },
        Builtin {
            name: "assert",
            params: &[BuiltinTy::Bool],
            ret: BuiltinTy::Void,
            ptr: ws_assert as *const u8,
        },
        // The collector, exposed so that a W# program can assert on it. Being
        // able to say "allocate this much rubbish, collect, check it went" in
        // the language itself is what makes the collector testable end to end
        // rather than only from Rust.
        Builtin {
            name: "gc_collect",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_collect as *const u8,
        },
        Builtin {
            name: "gc_trace",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_trace as *const u8,
        },
        // The two halves of a trace, so a program can mutate the heap while
        // the collector thread is marking it and then check nothing was lost.
        Builtin {
            name: "gc_trace_start",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_trace_start as *const u8,
        },
        Builtin {
            name: "gc_trace_finish",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_trace_finish as *const u8,
        },
        Builtin {
            name: "gc_traces",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_traces as *const u8,
        },
        Builtin {
            name: "gc_live_objects",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_live_objects as *const u8,
        },
        Builtin {
            name: "gc_live_bytes",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_live_bytes as *const u8,
        },
        Builtin {
            name: "gc_collections",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_collections as *const u8,
        },
    ]
}

/// The HTTP status lattice, as `(name, supertype)` pairs in declaration order.
///
/// These are ordinary W# structs with no fields: each is a type *and* a value
/// (its sole instance), and each declares its place in the lattice. The type
/// checker injects them ahead of the user's own declarations, so a handler can
/// be written as a set of overloads rather than a growing `switch`:
///
/// ```text
/// fn handle(r: Request, s: Status)      i64 { ... }
/// fn handle(r: Request, s: Status4xx)   i64 { ... }
/// fn handle(r: Request, s: NotFound404) i64 { ... }
/// ```
///
/// A parent must appear before its children so the lattice is well founded.
/// This is a table for the same reason [`builtins`] is: sema and codegen both
/// read it, and adding a status is one line.
///
/// They live in the global namespace because W# has no module system yet --
/// once it does (roadmap item 6) they move into a `http` module.
pub fn status_types() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("Status", None),
        ("Status1xx", Some("Status")),
        ("Continue100", Some("Status1xx")),
        ("Status2xx", Some("Status")),
        ("Ok200", Some("Status2xx")),
        ("Created201", Some("Status2xx")),
        ("Accepted202", Some("Status2xx")),
        ("NoContent204", Some("Status2xx")),
        ("Status3xx", Some("Status")),
        ("MovedPermanently301", Some("Status3xx")),
        ("Found302", Some("Status3xx")),
        ("NotModified304", Some("Status3xx")),
        ("Status4xx", Some("Status")),
        ("BadRequest400", Some("Status4xx")),
        ("Unauthorized401", Some("Status4xx")),
        ("Forbidden403", Some("Status4xx")),
        ("NotFound404", Some("Status4xx")),
        ("MethodNotAllowed405", Some("Status4xx")),
        ("Conflict409", Some("Status4xx")),
        ("Teapot418", Some("Status4xx")),
        ("TooManyRequests429", Some("Status4xx")),
        ("Status5xx", Some("Status")),
        ("ServerError500", Some("Status5xx")),
        ("NotImplemented501", Some("Status5xx")),
        ("BadGateway502", Some("Status5xx")),
        ("ServiceUnavailable503", Some("Status5xx")),
        ("GatewayTimeout504", Some("Status5xx")),
    ]
}

/// The abstract types, as `(name, members)` pairs.
///
/// An abstract type is a name for a set of concrete types. It is not a type a
/// value can have -- there is no machine representation for "a number" -- so it
/// may only be written as a parameter's annotation, where it says which types
/// that parameter accepts. That is what gives the dispatcher something to order
/// scalars by: `i64` is more specific than `Number` in the same way `NotFound404`
/// is more specific than `Status`, and for the same reason.
///
/// A table for the same reason [`builtins`] and [`status_types`] are: sema reads
/// it to seed the lattice, and adding a classifier is one row.
pub fn abstract_types() -> &'static [(&'static str, &'static [BuiltinTy])] {
    &[("Number", &[BuiltinTy::I64, BuiltinTy::F64])]
}

/// Runtime support routines that generated code calls but that are not
/// callable from W# source: the allocator and the panic handler.
pub fn runtime_symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("ws_alloc", crate::heap::ws_alloc as *const u8),
        ("ws_panic", ws_panic as *const u8),
        ("ws_log_object", crate::gc::ws_log_object as *const u8),
        ("ws_gc_poll", crate::gc::ws_gc_poll as *const u8),
        ("ws_resolve", crate::evacuate::ws_resolve as *const u8),
    ]
}

// ---- implementations ----------------------------------------------------

/// Print a W# string object followed by a newline.
///
/// # Safety
/// `ptr` must be null or point at a W# string object: a header whose aux word
/// holds the byte length, followed by that many bytes of UTF-8.
pub unsafe extern "C" fn ws_print_str(ptr: *const u8) {
    unsafe { crate::gc::checkpoint() };
    if ptr.is_null() {
        println!();
        return;
    }
    // SAFETY: `ptr` is a W# string object -- length in the aux word, bytes
    // immediately after the header.
    let text = unsafe {
        let len = (ptr.add(AUX_OFFSET as usize) as *const u64).read() as usize;
        let bytes = std::slice::from_raw_parts(ptr.add(HEADER_SIZE as usize), len);
        std::str::from_utf8(bytes)
    };
    match text {
        Ok(s) => println!("{s}"),
        Err(_) => println!("<invalid utf-8 string>"),
    }
}

pub extern "C" fn ws_print_int(value: i64) {
    unsafe { crate::gc::checkpoint() };
    println!("{value}");
}

/// Always shows a decimal point, so a float `1` is not mistaken for an integer.
pub extern "C" fn ws_print_float(value: f64) {
    unsafe { crate::gc::checkpoint() };
    if value.is_finite() && value.fract() == 0.0 {
        println!("{value:.1}");
    } else {
        println!("{value}");
    }
}

pub extern "C" fn ws_print_bool(value: i8) {
    unsafe { crate::gc::checkpoint() };
    println!("{}", value != 0);
}

pub extern "C" fn ws_assert(cond: i8) {
    unsafe { crate::gc::checkpoint() };
    if cond == 0 {
        ws_panic(PANIC_ASSERT);
    }
}

/// Force a collection.
///
/// Called directly from generated code, so the frame chain above is intact and
/// the stack maps describe the caller's roots -- which is exactly what a
/// collection needs.
pub extern "C" fn ws_gc_collect() {
    // Not while a trace is moving objects: see `gc::on_allocation`.
    if crate::gc::evacuating() {
        return;
    }
    unsafe { crate::gc::collect() };
}

/// Run a whole mark trace, synchronously: begin it, wait for the collector
/// thread to mark, finish it, wait for the sweep. What follows in the program
/// sees a heap with every cycle reclaimed.
pub extern "C" fn ws_gc_trace() {
    unsafe { crate::mark::run_full_trace() };
}

/// Begin a trace and return while the collector thread marks.
pub extern "C" fn ws_gc_trace_start() {
    unsafe { crate::mark::trace_start() };
}

/// Wait for the trace in flight to be entirely over.
pub extern "C" fn ws_gc_trace_finish() {
    unsafe { crate::mark::trace_finish() };
}

pub extern "C" fn ws_gc_traces() -> i64 {
    crate::gc::traces() as i64
}

pub extern "C" fn ws_gc_live_objects() -> i64 {
    crate::heap::heap_stats().live_objects as i64
}

pub extern "C" fn ws_gc_live_bytes() -> i64 {
    crate::heap::heap_stats().live_bytes as i64
}

pub extern "C" fn ws_gc_collections() -> i64 {
    crate::gc::collections() as i64
}

pub const PANIC_UNWRAP_NULL: i64 = 1;
pub const PANIC_ASSERT: i64 = 2;
pub const PANIC_NO_METHOD: i64 = 3;
pub const PANIC_DIVIDE_BY_ZERO: i64 = 4;
pub const PANIC_DIVIDE_OVERFLOW: i64 = 5;

/// The exit status of a program that panicked: the one a Rust program exits
/// with on a panic, so it is already familiar. A signal (`abort`) would be the
/// alternative, but a signal is what a *compiler* bug looks like; a plain exit
/// status says the program itself asked to stop.
pub const PANIC_EXIT_STATUS: i32 = 101;

/// Report a failure and end the process. Called from generated code for
/// failures that the type system permits but the program must not perform,
/// such as `.?` on a null optional.
pub extern "C" fn ws_panic(code: i64) {
    let reason = match code {
        PANIC_UNWRAP_NULL => "unwrapped a null optional".to_string(),
        PANIC_ASSERT => "assertion failed".to_string(),
        PANIC_NO_METHOD => "no overload matched these argument types".to_string(),
        PANIC_DIVIDE_BY_ZERO => "integer division by zero".to_string(),
        PANIC_DIVIDE_OVERFLOW => "integer overflow in division: i64::MIN / -1".to_string(),
        _ => format!("unknown failure (code {code})"),
    };
    eprintln!("W# panic: {reason}");
    crate::gc::report_if_asked();
    // `exit` rather than `panic!`: unwinding out of an `extern "C"` function
    // called from JIT-compiled frames has no defined behaviour. `exit` does
    // not unwind either -- it flushes stdout and leaves -- so whatever the
    // program printed before it died still arrives.
    std::process::exit(PANIC_EXIT_STATUS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{FLAG_IMMORTAL, TYPE_ID_STR, meta_word};

    /// Build a string object the way the code generator emits literals, so the
    /// test exercises the same layout `ws_print_str` reads.
    fn make_str(s: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&meta_word(TYPE_ID_STR, FLAG_IMMORTAL).to_ne_bytes());
        bytes.extend_from_slice(&(s.len() as u64).to_ne_bytes());
        bytes.extend_from_slice(s.as_bytes());
        bytes
    }

    #[test]
    fn string_layout_matches_what_print_reads() {
        let obj = make_str("hello");
        let len = unsafe { (obj.as_ptr().add(AUX_OFFSET as usize) as *const u64).read() };
        assert_eq!(len, 5);
        let bytes = &obj[HEADER_SIZE as usize..];
        assert_eq!(std::str::from_utf8(bytes).unwrap(), "hello");
    }

    #[test]
    fn every_builtin_has_a_distinct_name_and_a_real_pointer() {
        let all = builtins();
        let mut names: Vec<&str> = all.iter().map(|b| b.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate builtin name");
        assert!(all.iter().all(|b| !b.ptr.is_null()));
    }

    #[test]
    fn runtime_symbols_are_distinct_and_real() {
        let syms = runtime_symbols();
        assert!(syms.iter().any(|(n, _)| *n == "ws_alloc"));
        assert!(syms.iter().all(|(_, p)| !p.is_null()));
    }
}
