//! File and standard-input operations.
//!
//! These are the first builtins that can *fail*, and so the first that return
//! an error union. A `!str` is a tag and a payload, which crosses the boundary
//! as the two words a `#[repr(C)]` pair is returned in -- the same registers
//! the C ABI would use for a pair of words, which is what makes the two sides
//! agree without either knowing about the other.
//!
//! The tag is an index into the program's error table plus one, with zero
//! meaning success. That is why the table's first entries are fixed: see
//! [`crate::builtins::builtin_errors`].

use crate::strings::{alloc_str, str_bytes};

/// A `!str` as it crosses the boundary: the tag in a whole word, then the
/// payload. W# narrows the tag on the way in.
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
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_io_read_file(path: *const u8) -> FallibleStr {
    unsafe { crate::gc::checkpoint() };
    // Read before allocating: the allocation is a safepoint, and `path` is a
    // Rust local that no stack map describes.
    let path = String::from_utf8_lossy(unsafe { str_bytes(path) }).into_owned();
    match std::fs::read(&path) {
        Ok(bytes) => FallibleStr::ok(alloc_str(&bytes)),
        Err(e) => FallibleStr::err(io_error_tag(&e)),
    }
}

/// One line from standard input, without its newline.
///
/// End of input is `error.EndOfFile` rather than an empty string, which a
/// blank line also is.
pub extern "C" fn ws_io_read_line() -> FallibleStr {
    unsafe { crate::gc::checkpoint() };
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => FallibleStr::err(crate::builtins::ERROR_END_OF_FILE),
        Ok(_) => {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            FallibleStr::ok(alloc_str(trimmed.as_bytes()))
        }
        Err(e) => FallibleStr::err(io_error_tag(&e)),
    }
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
