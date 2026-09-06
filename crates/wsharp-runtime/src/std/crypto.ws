// The system's random bytes.
//
// The generator is the kernel's, and that is the whole design. A TLS stack is
// the last place to be clever about entropy: the kernel has it, it reseeds
// across a fork and across a VM being restored from a snapshot, and nothing in
// this process could know that either had happened. `getrandom` on Linux,
// `arc4random_buf` on the BSDs, `BCryptGenRandom` on Windows.
//
// One wrapper, because the builtin answers with a `str` -- a builtin may not
// allocate an array, since `array.new` is lowered inline and only the call site
// knows the element type -- and every caller wants a buffer.
const bytes = @import("std/bytes");

/// `n` random bytes.
///
/// The nonce of every AEAD in `std/cipher` and the private half of every key
/// exchange comes from here. A nonce that repeats under one key is not a
/// degraded AEAD, it is no AEAD at all, which is why this is a wrapper over the
/// system rather than anything of ours.
pub fn random(n: i64) ![]u8 {
    return bytes.of(try raw_random(n));
}
