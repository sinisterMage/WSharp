//! `[]u8`, and the bridge between it and `str`.
//!
//! A `str` and a `[]u8` are the same three things in memory: a header, an
//! `aux` word holding the count, and the bytes inline. Item 9's packing is
//! what makes that true -- a scalar takes its natural width, so a byte array's
//! stride is one -- and it is why the bridge below is a `memcpy` rather than a
//! conversion. What differs is what the language lets you do with each: a
//! `str` is immutable, so building one a byte at a time is quadratic, while a
//! `[]u8` is a buffer a hash can work in.
//!
//! These are the first builtins that *write* into an object they were handed.
//! The rule they sit inside is unchanged -- "a builtin may read and write
//! bytes; anything that moves a *reference* from one object into another is
//! written in W#" -- because a `u8` is not a reference and nothing here can
//! create a stale one. What a builtin still may not do is *allocate* an array:
//! `array.new` is lowered inline because only the call site knows the element
//! type, and so the stride and the type id to stamp. So every entry point here
//! is W# allocating and Rust filling.

use crate::builtins::ws_panic_index;
use crate::strings::{alloc_str, str_bytes};

/// The elements of a `[]u8`.
///
/// `str_bytes` under a name that says which shape is meant: the two coincide
/// exactly, and reading a byte array as a string's payload is not a
/// coincidence to be relied on quietly.
///
/// # Safety
/// `ptr` must be null or point at a W# `[]u8`.
pub(crate) unsafe fn elements<'a>(ptr: *const u8) -> &'a [u8] {
    unsafe { str_bytes(ptr) }
}

/// Check that `n` bytes at `at` are inside an object of `len` elements, and
/// panic through the same entry point `a[i]` uses if they are not.
///
/// A write that does not fit is reported rather than clamped. The precedent is
/// `byte_at` rather than `substr`: a short *read* is usually a computed range
/// that came up empty, and a short *write* is a buffer half filled with the
/// wrong thing, which in this file's callers is a hash of the wrong bytes.
pub(crate) fn check_span(at: i64, n: i64, len: i64) {
    if at < 0 || n < 0 || at + n > len {
        let offending = if at < 0 { at } else { at + n - 1 };
        ws_panic_index(offending, len);
    }
}

/// The bytes of `b[from..to]`, as a `str`.
///
/// Clamped rather than panicking, which is `str.substr`'s rule and is here for
/// `substr`'s reason: a slice is usually computed from a search that may have
/// found nothing, and every caller would otherwise have to check first.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `b` must be null or
/// point at a W# `[]u8`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_bytes_to_str(b: *const u8, from: i64, to: i64) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let elems = unsafe { elements(b) };
    let len = elems.len() as i64;
    let start = from.clamp(0, len) as usize;
    let end = to.clamp(start as i64, len) as usize;
    // Copy out before allocating: `alloc_str` is a safepoint, and after it `b`
    // may name an object that has moved.
    let owned = elems[start..end].to_vec();
    alloc_str(&owned)
}

/// Write every byte of `src` into `dst` starting at `at`.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `dst` must be null or
/// point at a W# `[]u8` and `src` null or a W# `str`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_bytes_from_str(dst: *mut u8, at: i64, src: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let src_len = unsafe { str_bytes(src) }.len() as i64;
    let dst_len = unsafe { elements(dst) }.len() as i64;
    check_span(at, src_len, dst_len);
    // Nothing here allocates, so nothing can move between these two reads.
    unsafe {
        std::ptr::copy_nonoverlapping(
            str_bytes(src).as_ptr(),
            dst.add(crate::header::HEADER_SIZE as usize + at as usize),
            src_len as usize,
        );
    }
}

/// Move `n` bytes from `src[src_at..]` into `dst[dst_at..]`.
///
/// `memmove`, not `memcpy`: `dst` and `src` may be the same array, which is
/// what a buffer shifting its own tail forward does.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; both must be null or
/// point at a W# `[]u8`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_bytes_copy(
    dst: *mut u8,
    dst_at: i64,
    src: *const u8,
    src_at: i64,
    n: i64,
) {
    unsafe { crate::gc::checkpoint() };
    let dst_len = unsafe { elements(dst) }.len() as i64;
    let src_len = unsafe { elements(src) }.len() as i64;
    check_span(dst_at, n, dst_len);
    check_span(src_at, n, src_len);
    let header = crate::header::HEADER_SIZE as usize;
    unsafe {
        std::ptr::copy(
            src.add(header + src_at as usize),
            dst.add(header + dst_at as usize),
            n as usize,
        );
    }
}

/// Whether two byte arrays hold the same bytes, in time that does not depend on
/// where they first differ.
///
/// The lengths are compared first and early, which is not a leak: a MAC tag's
/// length is a constant of the protocol, and a caller comparing two of a
/// different length has already made a different mistake. What must not vary
/// is *where* two equal-length buffers diverge, which is what a tag forgery
/// attempt measures.
///
/// This is a best effort rather than a guarantee, and the whole of `std/cipher`
/// depends on it being read that way: W# compiles through Cranelift, which is
/// free to turn a branchless expression into a branch.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; both must be null or
/// point at a W# `[]u8`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_bytes_equal(a: *const u8, b: *const u8) -> bool {
    unsafe { crate::gc::checkpoint() };
    let (a, b) = unsafe { (elements(a), elements(b)) };
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}
