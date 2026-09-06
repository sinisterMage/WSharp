//! The process's own arguments and environment.
//!
//! The command line is the first thing a program written in W# needs that W#
//! could not say: `main` takes no arguments, by the type checker's own rule,
//! and `Jit::run` passes it the one word every W# function takes, which is the
//! closure environment pointer rather than an `argc`. So the arguments arrive
//! the way the type registry and the stack maps do -- published once into
//! process-wide storage before any generated code runs, and read from there.
//!
//! That is the third thing in this runtime allowed to be process-wide, and it
//! qualifies for the same reason the other two do: it is frozen before the
//! program starts and never written again.

use crate::strings::{alloc_str, str_bytes};
use crate::sys;
use std::sync::OnceLock;

/// The arguments this program was given, without the executable's own name.
///
/// Set by whichever driver compiled the program -- `wsharp run` after its own
/// flags, `ingot` from its whole command line -- and never after that.
static ARGS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

/// Publish the command line. Called once, before the program runs.
///
/// A second call is ignored rather than reported: there is one program per
/// process and nothing sensible to do with a second answer.
pub fn set_args(args: Vec<Vec<u8>>) {
    let _ = ARGS.set(args);
}

/// `?str`, as it crosses the boundary: the tag in a whole word, then the value.
///
/// Zero is null and one is a value, which is what [`crate::repr`]'s option tag
/// means -- the same shape the broker's `raw_poll` answers with.
#[repr(C)]
pub struct MaybeStr {
    pub tag: i64,
    pub value: *mut u8,
}

/// The command line, as a blob of length-prefixed arguments.
///
/// A `str` for the reason `fs.raw_read_dir` is one, and the same four-byte
/// big-endian framing: a builtin may not allocate an array, so `std/os.args`
/// cuts the blob up in W#.
///
/// Infallible. A program always has a command line, and one with no arguments
/// has an empty one, which is an empty blob rather than an error.
pub extern "C" fn ws_os_raw_args() -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let empty: Vec<Vec<u8>> = Vec::new();
    let args = ARGS.get().unwrap_or(&empty);
    alloc_str(&crate::fs::length_prefixed(args))
}

/// One environment variable, or null.
///
/// Not an error union: a variable that is not set is the ordinary case, and
/// `orelse` is what a caller wants to write. `HOME` on Unix and `USERPROFILE`
/// on Windows are what a store's location is worked out from, and asking for
/// the wrong one of those is not a failure either.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `name` must be null or
/// point at a W# string object, and `out` must point at storage laid out as a
/// [`MaybeStr`].
pub unsafe extern "C" fn ws_os_env(out: *mut MaybeStr, name: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let name = unsafe { str_bytes(name) }.to_vec();
    // Reading the environment is a memory access rather than a syscall, but it
    // is the platform's memory: the safe region costs nothing and keeps every
    // call through `sys` under one rule.
    let found = crate::worker::blocking(|| sys::env(&name));
    let result = match found {
        Some(value) => MaybeStr {
            tag: 1,
            value: alloc_str(&value),
        },
        None => MaybeStr {
            tag: 0,
            value: std::ptr::null_mut(),
        },
    };
    unsafe { out.write(result) };
}
