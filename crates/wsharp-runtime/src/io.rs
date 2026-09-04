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

use crate::strings::{alloc_str, str_bytes};

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
    fn ok(value: *mut u8) -> FallibleStr {
        FallibleStr { tag: 0, value }
    }

    fn err(tag: i64) -> FallibleStr {
        FallibleStr {
            tag,
            value: std::ptr::null_mut(),
        }
    }
}

/// The whole contents of a file.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects, and `out` must point at writable
/// storage laid out as a [`FallibleStr`].
pub unsafe extern "C" fn ws_io_read_file(out: *mut FallibleStr, path: *const u8) {
    unsafe { crate::gc::checkpoint() };
    // Read before allocating: the allocation is a safepoint, and `path` is a
    // Rust local that no stack map describes.
    let path = String::from_utf8_lossy(unsafe { str_bytes(path) }).into_owned();
    let result = match std::fs::read(&path) {
        Ok(bytes) => FallibleStr::ok(alloc_str(&bytes)),
        Err(e) => FallibleStr::err(io_error_tag(&e)),
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
pub unsafe extern "C" fn ws_io_read_line(out: *mut FallibleStr) {
    unsafe { crate::gc::checkpoint() };
    let mut line = String::new();
    let result = match std::io::stdin().read_line(&mut line) {
        Ok(0) => FallibleStr::err(crate::builtins::ERROR_END_OF_FILE),
        Ok(_) => {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            FallibleStr::ok(alloc_str(trimmed.as_bytes()))
        }
        Err(e) => FallibleStr::err(io_error_tag(&e)),
    };
    unsafe { out.write(result) };
}

/// Write `contents` to `path`, replacing what was there.
///
/// `!void` is a tag with no payload, so this returns one word.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_io_write_file(path: *const u8, contents: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let path = String::from_utf8_lossy(unsafe { str_bytes(path) }).into_owned();
    let bytes = unsafe { str_bytes(contents) }.to_vec();
    match std::fs::write(&path, &bytes) {
        Ok(()) => 0,
        Err(e) => io_error_tag(&e),
    }
}

/// Whether a path exists at all, which is a question with no failure case.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_io_exists(path: *const u8) -> bool {
    unsafe { crate::gc::checkpoint() };
    let path = String::from_utf8_lossy(unsafe { str_bytes(path) }).into_owned();
    std::path::Path::new(&path).exists()
}

/// Which error a failure is, as a tag.
///
/// Only the distinctions a program can act on: whether the file was there, and
/// whether it was allowed to look. Everything else is one error, because a
/// caller that wanted more detail could not have got it portably anyway.
fn io_error_tag(e: &std::io::Error) -> i64 {
    match e.kind() {
        std::io::ErrorKind::NotFound => crate::builtins::ERROR_NOT_FOUND,
        std::io::ErrorKind::PermissionDenied => crate::builtins::ERROR_PERMISSION_DENIED,
        _ => crate::builtins::ERROR_IO_FAILED,
    }
}
