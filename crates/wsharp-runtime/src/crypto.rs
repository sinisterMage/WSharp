//! The system's random bytes, and the wall clock.
//!
//! Two builtins that have nothing in common except that item 10 cannot be
//! written without either. A nonce that repeats under one key breaks every
//! AEAD in `std/cipher` completely, and a certificate's validity is a question
//! about the date.
//!
//! Neither is generated here. The generator is the kernel's because it has the
//! entropy and because it knows things nothing in this process can -- when the
//! machine forked, when the VM was restored from a snapshot. The clock is the
//! system's for the ordinary reason.

use crate::io::FallibleStr;
use crate::sys;

/// `n` random bytes, as a `str`.
///
/// A `str` rather than a `[]u8` because a builtin may not allocate an array --
/// `array.new` is lowered inline, since only the call site knows the element
/// type -- and a `str` is the one byte-shaped object the runtime can make.
/// `std/crypto.bytes` converts it, which costs the one copy `bytes.of` costs.
///
/// Inside a safe region, because `getrandom` blocks until the kernel's pool is
/// initialised. That is a real wait exactly once, in the first seconds of a
/// boot, and waiting is the right answer: the alternative is a key made of
/// whatever was lying around.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `out` must point at
/// storage laid out as a [`FallibleStr`].
pub unsafe extern "C" fn ws_crypto_random(out: *mut FallibleStr, n: i64) {
    unsafe { crate::gc::checkpoint() };
    let n = n.clamp(0, MAX_RANDOM) as usize;
    let mut buf = vec![0u8; n];
    let filled = crate::worker::blocking(|| sys::random(&mut buf));
    let result = match filled {
        // Allocating outside the region, which is what lets it be a safepoint
        // like any other.
        Ok(()) => FallibleStr::ok(crate::strings::alloc_str(&buf)),
        Err(e) => FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// A single request is capped for [`crate::net`]'s reason: a program asking
/// for a gigabyte should not get a gigabyte of zeroed buffer first. Nothing in
/// a TLS handshake wants more than 32 bytes at a time.
const MAX_RANDOM: i64 = 1 << 20;

/// Seconds since the Unix epoch.
///
/// Infallible, and it says so: every arm's call either answers or has no way
/// to report that it did not. A clock that is simply wrong is not a failure
/// this layer can detect, and a caller checking a certificate's validity is
/// already the thing that decides whether the answer is believable.
pub extern "C" fn ws_time_now() -> i64 {
    unsafe { crate::gc::checkpoint() };
    sys::wall_clock_secs()
}
