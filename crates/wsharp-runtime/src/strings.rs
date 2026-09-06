//! The standard library's string and array operations.
//!
//! These are the first builtins that *allocate*, which makes them the first
//! that are safepoints: `ws_alloc` runs `on_allocation`, which can start or
//! finish a collection and move objects, while a builtin's arguments live in
//! Rust locals that no stack map describes. Every function here therefore
//! copies what it needs into plain bytes *before* allocating, and never holds
//! a raw pointer across a call to the allocator.
//!
//! What is *not* here is as deliberate. A string holds no references, so
//! nothing in this file can create a stale one. Anything that assembles an
//! object holding references -- `concat` on an array, `split` returning
//! `[]str` -- is written in W# instead (`std/array.ws`, `std/str.ws`), where
//! the write barrier, the load barrier and the stack maps all apply by
//! construction. Copying references by hand here would mean reproducing all
//! three, and getting one of them wrong is not a failure that shows up as a
//! failing test.

use crate::header::{AUX_OFFSET, HEADER_SIZE, TYPE_ID_STR};
use crate::heap::ws_alloc;

/// The bytes of a W# string object.
///
/// # Safety
/// `ptr` must be a live string object, or null.
pub unsafe fn str_bytes<'a>(ptr: *const u8) -> &'a [u8] {
    if ptr.is_null() {
        return &[];
    }
    unsafe {
        let len = (ptr.add(AUX_OFFSET as usize) as *const u64).read() as usize;
        std::slice::from_raw_parts(ptr.add(HEADER_SIZE as usize), len)
    }
}

/// Allocate a string object holding `bytes`.
///
/// The allocation happens first and the copy second, which is the order that
/// matters: nothing of ours is live across it.
pub fn alloc_str(bytes: &[u8]) -> *mut u8 {
    let size = HEADER_SIZE as u64 + bytes.len() as u64;
    let obj = ws_alloc(TYPE_ID_STR, size, bytes.len() as u64);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), obj.add(HEADER_SIZE as usize), bytes.len());
    }
    obj
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_str_len(s: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    unsafe { str_bytes(s).len() as i64 }
}

/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_str_concat(a: *const u8, b: *const u8) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    // Copy both sides out before allocating: the allocation is a safepoint,
    // and after it `a` and `b` may name objects that have moved.
    let mut bytes = Vec::with_capacity(unsafe { str_bytes(a).len() + str_bytes(b).len() });
    bytes.extend_from_slice(unsafe { str_bytes(a) });
    bytes.extend_from_slice(unsafe { str_bytes(b) });
    alloc_str(&bytes)
}

/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_str_eq(a: *const u8, b: *const u8) -> bool {
    unsafe { crate::gc::checkpoint() };
    unsafe { str_bytes(a) == str_bytes(b) }
}

/// `s[from..to]`, clamped to the string rather than trapping.
///
/// Clamping rather than panicking because a slice is usually computed from a
/// `find` that may have returned nothing, and every caller would otherwise
/// have to bounds-check before asking.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_str_substr(s: *const u8, from: i64, to: i64) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let bytes = unsafe { str_bytes(s) };
    let len = bytes.len() as i64;
    let from = from.clamp(0, len) as usize;
    let to = to.clamp(from as i64, len) as usize;
    let slice = bytes[from..to].to_vec();
    alloc_str(&slice)
}

/// The byte index of the first `needle`, or -1.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string arguments
/// must be null or point at W# string objects.
pub unsafe extern "C" fn ws_str_find(s: *const u8, needle: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let hay = unsafe { str_bytes(s) };
    let needle = unsafe { str_bytes(needle) };
    if needle.is_empty() {
        return 0;
    }
    if needle.len() > hay.len() {
        return -1;
    }
    for start in 0..=hay.len() - needle.len() {
        if &hay[start..start + needle.len()] == needle {
            return start as i64;
        }
    }
    -1
}

pub extern "C" fn ws_str_from_int(value: i64) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    alloc_str(value.to_string().as_bytes())
}

pub extern "C" fn ws_str_from_float(value: f64) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    alloc_str(format!("{value}").as_bytes())
}

/// How many elements an array holds.
///
/// The one array operation that belongs here: it reads the header and touches
/// no reference, so it is the same machine code whatever the elements are.
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `a` must be null or
/// point at a W# array object.
pub unsafe extern "C" fn ws_array_len(a: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    if a.is_null() {
        return 0;
    }
    unsafe { (a.add(AUX_OFFSET as usize) as *const u64).read() as i64 }
}

/// The byte at `i`, as a number in 0..256.
///
/// Bounds-checked, and it panics through the same entry point an array index
/// does, so a mistake here reads exactly like a mistake there. This is what
/// makes a protocol parseable in W#: without it a `str` can be compared,
/// searched and sliced but never looked *into*.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string argument
/// must be null or point at a W# string object.
pub unsafe extern "C" fn ws_str_byte_at(s: *const u8, i: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let bytes = unsafe { str_bytes(s) };
    let len = bytes.len() as i64;
    if i < 0 || i >= len {
        crate::builtins::ws_panic_index(i, len);
    }
    i64::from(bytes[i as usize])
}

/// A one-byte string. The other half of `byte_at`: what takes a protocol apart
/// has to be able to put one together.
///
/// Anything outside 0..256 is taken modulo 256, because a `str` is bytes and
/// there is no other answer that is still a byte.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_str_from_byte(b: i64) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    alloc_str(&[(b.rem_euclid(256)) as u8])
}

/// The decimal number a string spells.
///
/// Strict: an empty string, a stray sign, a trailing space or a digit that is
/// not one is `error.BadFormat` rather than a partial answer. A parser that
/// wants to be lenient can be; one that wants to be strict cannot recover
/// leniency.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `out` must point at
/// storage laid out as a [`crate::io::FallibleI64`].
pub unsafe extern "C" fn ws_str_parse_int(out: *mut crate::io::FallibleI64, s: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let bytes = unsafe { str_bytes(s) };
    let result = match parse_decimal(bytes) {
        Some(n) => crate::io::FallibleI64::ok(n),
        None => crate::io::FallibleI64::err(crate::builtins::ERROR_BAD_FORMAT),
    };
    unsafe { out.write(result) };
}

/// `-?[0-9]+`, and nothing else. Overflow is a failure rather than a wrap:
/// a length header that does not fit in an `i64` is not a length.
fn parse_decimal(bytes: &[u8]) -> Option<i64> {
    let (negative, digits) = match bytes.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, bytes),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for byte in digits {
        let digit = byte.checked_sub(b'0').filter(|d| *d < 10)?;
        value = value.checked_mul(10)?.checked_add(i64::from(digit))?;
    }
    if negative { Some(-value) } else { Some(value) }
}

/// ASCII lowercase, byte for byte.
///
/// Only ASCII, deliberately: a header name is ASCII by definition, and case
/// folding anything else needs a Unicode table this runtime does not have and
/// should not grow for the sake of `Content-Length`.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string argument
/// must be null or point at a W# string object.
pub unsafe extern "C" fn ws_str_to_lower(s: *const u8) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let lowered: Vec<u8> = unsafe { str_bytes(s) }
        .iter()
        .map(|b| b.to_ascii_lowercase())
        .collect();
    alloc_str(&lowered)
}

/// Without leading or trailing spaces, tabs, carriage returns or newlines.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; the string argument
/// must be null or point at a W# string object.
pub unsafe extern "C" fn ws_str_trim(s: *const u8) -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let bytes = unsafe { str_bytes(s) };
    let is_space = |b: &u8| matches!(*b, b' ' | b'\t' | b'\r' | b'\n');
    let start = bytes.iter().position(|b| !is_space(b));
    let trimmed = match start {
        None => &[][..],
        Some(start) => {
            let end = bytes.iter().rposition(|b| !is_space(b)).unwrap_or(start);
            &bytes[start..=end]
        }
    };
    let owned = trimmed.to_vec();
    alloc_str(&owned)
}
