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

/// Runtime support routines that generated code calls but that are not
/// callable from W# source: the allocator and the panic handler.
pub fn runtime_symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("ws_alloc", crate::heap::ws_alloc as *const u8),
        ("ws_panic", ws_panic as *const u8),
        ("ws_log_object", crate::gc::ws_log_object as *const u8),
        ("ws_gc_poll", crate::gc::ws_gc_poll as *const u8),
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
    unsafe { crate::gc::collect() };
}

/// Force a backup mark trace, which reclaims cycles.
pub extern "C" fn ws_gc_trace() {
    unsafe { crate::gc::trace() };
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
pub const PANIC_UNREACHABLE: i64 = 3;
pub const PANIC_NO_METHOD: i64 = 4;

/// Abort with a message. Called from generated code for failures that the type
/// system permits but the program must not perform, such as `.?` on a null
/// optional.
pub extern "C" fn ws_panic(code: i64) {
    let reason = match code {
        PANIC_UNWRAP_NULL => "unwrapped a null optional",
        PANIC_ASSERT => "assertion failed",
        PANIC_UNREACHABLE => "reached unreachable code",
        PANIC_NO_METHOD => "no overload matched these argument types",
        _ => "unknown failure",
    };
    eprintln!("W# panic: {reason}");
    // `abort` rather than `panic!`: unwinding out of an `extern "C"` function
    // called from JIT-compiled frames has no defined behaviour.
    std::process::abort();
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
