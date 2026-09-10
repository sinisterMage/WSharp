//! Directories, and the two facts about a path that a store needs.
//!
//! `std/io` is four functions about a file's *contents*. This is the rest of a
//! filesystem: somewhere to put things, a way to list what is there, `rename`,
//! which is what makes an install atomic, and enough of a `stat` to tell a
//! directory from a file and say how big one is.
//!
//! There is a `modified_at`, and it is the one question here that a system may
//! make [`crate::sys`] read a `struct stat` for -- whose layout differs across
//! the BSD family and is a versioned symbol on Linux, and which every other
//! question here is deliberately shaped to avoid. It is asked anyway because a
//! watcher that had to hash a whole tree on every tick is a real cost rather
//! than an inconvenience, and because two of the three arms answer it without a
//! layout at all. `crate::sys::modified_at` says what each one pays.
//!
//! A *store* still answers "has this changed?" by hashing rather than by
//! comparing timestamps: a checkout does not preserve them and two machines do
//! not agree about them. That was never the reason this call was missing, and
//! it is still the reason `ingot` does not use it.
//!
//! There is still no mode *reader*: "will this start" is answerable with
//! `access`, "which bits are set" is not, and only the first has a caller.
//!
//! Every call is made inside a safe region, so a slow or remote filesystem
//! cannot stall this worker's collector. Each follows the shape
//! [`crate::io::ws_io_read_file`] set: copy the path out of the heap before
//! blocking *and* before allocating, allocate outside the region, and write the
//! out-parameter last.

use crate::io::{FallibleI64, FallibleStr};
use crate::strings::{alloc_str, str_bytes};
use crate::sys;

/// Create one directory. Its parent must already be there.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_mkdir(path: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    match crate::worker::blocking(|| sys::mkdir(&path)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// Remove one empty directory.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_rmdir(path: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    match crate::worker::blocking(|| sys::rmdir(&path)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// Remove one file.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_remove(path: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    match crate::worker::blocking(|| sys::remove(&path)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// Move a path, replacing whatever was at the destination.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; both arguments must be
/// null or point at W# string objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_rename(from: *const u8, to: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let from = unsafe { str_bytes(from) }.to_vec();
    let to = unsafe { str_bytes(to) }.to_vec();
    match crate::worker::blocking(|| sys::rename(&from, &to)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// Set a path's permission bits.
///
/// `mode` is an `i64` because that is the only integer a W# literal is, and it
/// is written the way C would write it: `fs.chmod(p, 0o755)`. A negative one is
/// refused rather than wrapped -- `as u32` on a negative would set every bit,
/// and the one thing worse than failing to make a file executable is making it
/// setuid by accident.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_chmod(path: *const u8, mode: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    let Ok(mode) = u32::try_from(mode) else {
        return crate::builtins::ERROR_IO_FAILED;
    };
    match crate::worker::blocking(|| sys::chmod(&path, mode)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// Whether this process may run a path.
///
/// No failure case, for `is_dir`'s reason: a path that is not there cannot be
/// run, and neither can one that cannot be reached.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_is_executable(path: *const u8) -> bool {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    crate::worker::blocking(|| sys::is_executable(&path))
}

/// Whether a path names a directory.
///
/// No failure case, exactly as `io.exists` has none: a path that is not there
/// is not a directory, and so is one that cannot be read.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_is_dir(path: *const u8) -> bool {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    crate::worker::blocking(|| sys::is_dir(&path))
}

/// How many bytes a file holds.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object, and `out` must point at storage laid out as a
/// [`FallibleI64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_size(out: *mut FallibleI64, path: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    let result = match crate::worker::blocking(|| sys::file_size(&path)) {
        Ok(n) => FallibleI64::ok(n),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// When a path was last written, in seconds since the Unix epoch.
///
/// The same clock `time.now()` reads, so "is this newer than when I looked" is
/// a subtraction rather than a conversion.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object, and `out` must point at storage laid out as a
/// [`FallibleI64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_modified_at(out: *mut FallibleI64, path: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    let result = match crate::worker::blocking(|| sys::modified_at(&path)) {
        Ok(n) => FallibleI64::ok(n),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// What a directory holds, as a blob of length-prefixed names.
///
/// A `str` rather than a `[]str` because a builtin may not allocate an array --
/// `array.new` is lowered inline, since only the call site knows the element
/// type. `std/fs.read_dir` cuts it back up, which is the same split
/// `crypto.raw_system_roots` and `std/x509.split_blob` already use, and for the
/// same reason: the runtime hands over bytes and W# builds the objects.
///
/// Four-byte big-endian lengths rather than a separator byte, so that the
/// encoding says nothing about what a name may contain and an empty listing is
/// an empty blob rather than something to special-case.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `path` must be null or
/// point at a W# string object, and `out` must point at storage laid out as a
/// [`FallibleStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_fs_raw_read_dir(out: *mut FallibleStr, path: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    let result = match crate::worker::blocking(|| sys::read_dir(&path)) {
        Ok(names) => FallibleStr::ok(alloc_str(&length_prefixed(&names))),
        Err(e) => FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Each entry as a four-byte big-endian length followed by its bytes.
pub(crate) fn length_prefixed(items: &[Vec<u8>]) -> Vec<u8> {
    let mut blob = Vec::new();
    for item in items {
        blob.extend_from_slice(&(item.len() as u32).to_be_bytes());
        blob.extend_from_slice(item);
    }
    blob
}
