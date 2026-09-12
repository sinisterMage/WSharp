//! File and standard-input operations.
//!
//! These are the first builtins that can *fail*, and so the first that produce
//! an error union. A `!str` is a tag and a payload, and it crosses the boundary
//! through a pointer the caller passes rather than as a returned value.
//!
//! That indirection is not decoration. A two-word `#[repr(C)]` struct is
//! returned in RAX:RDX under System V and in X0:X1 under AArch64, but Windows
//! x64 returns any aggregate wider than one word through a hidden pointer,
//! shifting every real argument one register along. Generated code cannot
//! declare a shape that satisfies all three, and guessing wrong is silent:
//! `read_file` wrote its result through whatever address its first argument
//! happened to be -- a string literal, which lives in read-only memory -- and
//! the process died with no message at all. Parameters carry this shape
//! identically everywhere, so passing the destination explicitly leaves no ABI
//! anything to infer.
//!
//! The tag is an index into the program's error table plus one, with zero
//! meaning success. That is why the table's first entries are fixed: see
//! [`crate::builtins::builtin_errors`].
//!
//! The operating system is reached through [`crate::sys`], which declares it by
//! hand, rather than through Rust's `std::fs` and `std::io`. What that buys is
//! `errno` instead of a portable approximation of it, and one layer instead of
//! two once sockets arrive. What it costs is that path encoding, `EINTR` and
//! short reads are now ours to get right; they are handled in `sys`, once.
//!
//! Every call here is made inside a safe region, so a slow disk or a terminal
//! nobody is typing at cannot stall this worker's collector.

use crate::strings::{alloc_str, str_bytes};
use crate::sys;

/// A `!str` as it crosses the boundary: the tag in a whole word, then the
/// payload. W# narrows the tag on the way in.
///
/// Generated code hands over the address of a slot with this layout; the
/// offsets it loads from afterwards are this struct's, so the `#[repr(C)]` is
/// load-bearing.
#[repr(C)]
pub struct FallibleStr {
    pub tag: i64,
    pub value: *mut u8,
}

impl FallibleStr {
    pub(crate) fn ok(value: *mut u8) -> FallibleStr {
        FallibleStr { tag: 0, value }
    }

    pub(crate) fn err(tag: i64) -> FallibleStr {
        FallibleStr {
            tag,
            value: std::ptr::null_mut(),
        }
    }
}

/// A `!i64` as it crosses the boundary: the same two-word shape, for every
/// builtin whose answer is a number that might not exist.
#[repr(C)]
pub struct FallibleI64 {
    pub tag: i64,
    pub value: i64,
}

impl FallibleI64 {
    pub(crate) fn ok(value: i64) -> FallibleI64 {
        FallibleI64 { tag: 0, value }
    }

    pub(crate) fn err(tag: i64) -> FallibleI64 {
        FallibleI64 { tag, value: 0 }
    }
}

/// A `!f64` as it crosses the boundary.
///
/// The same two-word shape, and the reason it is written out rather than
/// generic: the code generator lays the destination out as one machine word
/// for the tag followed by the payload's own slot, so what the runtime writes
/// has to be a `#[repr(C)]` struct with exactly those two fields.
#[repr(C)]
pub struct FallibleF64 {
    pub tag: i64,
    pub value: f64,
}

impl FallibleF64 {
    pub(crate) fn ok(value: f64) -> FallibleF64 {
        FallibleF64 { tag: 0, value }
    }

    pub(crate) fn err(tag: i64) -> FallibleF64 {
        FallibleF64 { tag, value: 0.0 }
    }
}

/// The whole contents of a file.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects, and `out` must point at writable
/// storage laid out as a [`FallibleStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_io_read_file(out: *mut FallibleStr, path: *const u8) {
    unsafe { crate::gc::checkpoint() };
    // Copy the path out before allocating *and* before blocking: an allocation
    // is a safepoint, a safe region is a window in which a collection can run,
    // and `path` names a heap object that no stack map describes.
    //
    // The bytes are taken as they are rather than through a lossy conversion to
    // text. A W# `str` is arbitrary bytes and so is a Unix path.
    let path = unsafe { str_bytes(path) }.to_vec();
    let read = crate::worker::blocking(|| sys::read_file(&path));
    let result = match read {
        Ok(bytes) => FallibleStr::ok(alloc_str(&bytes)),
        Err(e) => FallibleStr::err(sys::error_tag(e)),
    };
    // After the allocation, never before: `out` points into the caller's frame,
    // which no stack map describes, so a reference parked there would be
    // invisible to a collection this function triggers.
    unsafe { out.write(result) };
}

/// One line from standard input, without its newline.
///
/// End of input is `error.EndOfFile` rather than an empty string, which a
/// blank line also is.
///
/// # Safety
/// `out` must point at writable storage laid out as a [`FallibleStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_io_read_line(out: *mut FallibleStr) {
    unsafe { crate::gc::checkpoint() };
    // The blocking call this whole mechanism exists for: a program waiting on a
    // terminal waits indefinitely, and its collector must not wait with it.
    let read = crate::worker::blocking(|| sys::read_line(sys::stdin()));
    let result = match read {
        Ok(Some(line)) => FallibleStr::ok(alloc_str(&line)),
        Ok(None) => FallibleStr::err(crate::builtins::ERROR_END_OF_FILE),
        Err(e) => FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Write `contents` to `path`, replacing what was there.
///
/// `!void` is a tag with no payload, so this returns one word.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_io_write_file(path: *const u8, contents: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    let bytes = unsafe { str_bytes(contents) }.to_vec();
    match crate::worker::blocking(|| sys::write_file(&path, &bytes)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// Whether a path exists at all, which is a question with no failure case.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_io_exists(path: *const u8) -> bool {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    crate::worker::blocking(|| sys::exists(&path))
}
/// Render a status line on stderr, replacing it only on a terminal.
///
/// # Safety
/// `message` points at a W# string. No heap pointer survives the safe region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_io_progress(message: *const u8, finished: i8) {
    use std::io::{IsTerminal, Write};
    unsafe { crate::gc::checkpoint() };
    let bytes = unsafe { crate::strings::str_bytes(message) }.to_vec();
    // Package names and URLs are external text, never terminal instructions.
    let message: String = String::from_utf8_lossy(&bytes)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    crate::worker::blocking(|| {
        let stderr = std::io::stderr();
        let terminal = stderr.is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb");
        let mut out = stderr.lock();
        if terminal {
            if !message.is_empty() {
                let _ = write!(out, "\r\x1b[2K{message}");
            }
            if finished != 0 {
                let _ = writeln!(out);
            }
        } else if !message.is_empty() {
            let _ = writeln!(out, "{message}");
        }
        let _ = out.flush();
    });
}
