// Fixed-width unsigned integers, and Montgomery arithmetic over them.
//
// This is the engine two very different things stand on: P-256's field, where
// every operation is on a secret and must take the same time whatever it is
// given, and RSA signature verification, where every operation is on a public
// certificate and none of it is secret at all. One implementation serves both
// because the constant-time discipline costs nothing here -- there is no
// shortcut worth taking in a modular multiplication anyway.
//
// **A limb is 32 bits wide, and that is the load-bearing decision.** W# has no
// 128-bit type and no 64x64 -> 128 product, which is the one thing item 9 left
// out and the one thing a bignum usually wants. With 32-bit limbs it is not
// wanted: the largest quantity anything below computes is
//
//     t + a*b + carry  <=  (2^32-1) + (2^32-1)^2 + (2^32-1)  =  2^64 - 1
//
// which is exactly a `u64` and not one bit more. So the whole module is
// ordinary W# arithmetic, and `bits.mulhi` -- which the roadmap reserved a
// place for -- is still not needed.
//
// A number is a `[]u32` of limbs, least significant first, whose *length is
// its width*: nothing here is variable-length, because everything above it
// works modulo a modulus of a size fixed when the key was read. Two numbers
// handed to one operation always have the same length.
//
// **Nothing here divides.** The one place a division is usually reached for is
// `R^2 mod n`, and that is computed by doubling instead -- 64*limbs doublings
// with a masked conditional subtract. RSA verification needs no remainder and
// neither does P-256, so a division would be code nobody calls in a file where
// being wrong is a security problem. That is the reason AES decryption is not
// written either.
const array = @import("std/array");

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------
//
// Every comparison below answers with all-ones or all-zeros rather than with a
// `bool`, because a `bool` wants an `if` and an `if` on a secret is the thing
// this file exists to avoid. The trick throughout is that a `u32` subtraction
// widened to `u64` cannot wrap, so bit 63 of the difference *is* the borrow.

/// All ones when `x < y`, all zeros otherwise.
fn lt_mask(x: u32, y: u32) u32 {
    const d = u64(x) - u64(y);
    return 0 - u32(d >> 63);
}

/// All ones when `x != 0`, all zeros otherwise.
fn nz_mask(x: u32) u32 { return lt_mask(0, x); }

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

/// A zeroed number of `limbs` limbs.
pub fn new(limbs: i64) []u32 {
    const out: []u32 = array.new(limbs);
    return out;
}

/// The `n` bytes of `b` at `at`, read as one big-endian number of `limbs`
/// limbs.
///
/// Big-endian because that is how every wire format this will ever be handed
/// writes an integer: a DER `INTEGER`, an RSA modulus, an EC coordinate.
pub fn from_be(b: []u8, at: i64, n: i64, limbs: i64) []u32 {
    const out = new(limbs);
    var i = 0;
    while (i < n) : (i += 1) {
        const limb = i / 4;
        if (limb < limbs) {
            out[limb] |= u32(b[at + n - 1 - i]) << u32((i % 4) * 8);
        }
    }
    return out;
}

/// `a` as `n` big-endian bytes, written into `out` at `at` -- left-padded with
/// zeros if `n` is wider than the number and truncated from the top if it is
/// narrower.
pub fn to_be_at(a: []u32, out: []u8, at: i64, n: i64) void {
    const limbs = array.len(a);
    var i = 0;
    while (i < n) : (i += 1) {
        var v: u8 = 0;
        const limb = i / 4;
        if (limb < limbs) { v = u8((a[limb] >> u32((i % 4) * 8)) & 0xff); }
        out[at + n - 1 - i] = v;
    }
    return;
}

/// `a` as big-endian bytes, filling `out` exactly.
pub fn to_be(a: []u32, out: []u8) void {
    to_be_at(a, out, 0, array.len(out));
    return;
}

/// `dst = src`, for numbers of the same width.
pub fn copy(dst: []u32, src: []u32) void {
    const k = array.len(dst);
    var i = 0;
    while (i < k) : (i += 1) { dst[i] = src[i]; }
    return;
}

/// True when every limb is zero. Constant time: the whole number is folded.
pub fn is_zero(a: []u32) bool {
    const k = array.len(a);
    var acc: u32 = 0;
    var i = 0;
    while (i < k) : (i += 1) { acc |= a[i]; }
    return acc == 0;
}

/// All ones when every limb of `a` is zero, all zeros otherwise.
///
/// A mask rather than the `bool` `is_zero` gives back, because the callers
/// that need this are choosing between two values without branching.
pub fn zero_mask(a: []u32) u32 {
    const k = array.len(a);
    var acc: u32 = 0;
    var i = 0;
    while (i < k) : (i += 1) { acc |= a[i]; }
    return ~nz_mask(acc);
}

/// `out = a` when `take` is all ones and `out = b` when it is all zeros.
pub fn select(out: []u32, a: []u32, b: []u32, take: u32) void {
    const k = array.len(out);
    var i = 0;
    while (i < k) : (i += 1) { out[i] = (a[i] & take) | (b[i] & ~take); }
    return;
}

/// Bit `i` of `a`, counting from the least significant.
pub fn bit(a: []u32, i: i64) u32 { return (a[i / 32] >> u32(i % 32)) & 1; }

/// The position of the highest set bit plus one, or zero for a zero number.
///
/// This one *is* variable time, and only the exponent is ever measured with
/// it -- a public number in every caller there is.
pub fn bit_len(a: []u32) i64 {
    var n = array.len(a) * 32;
    while (n > 0) {
        if (bit(a, n - 1) == 1) { return n; }
        n -= 1;
    }
    return 0;
}

/// -1, 0 or 1 as `a` is less than, equal to or greater than `b`.
///
/// Constant time: every limb is examined, and a more significant one simply
/// overrides what the less significant ones had decided.
pub fn cmp(a: []u32, b: []u32) i64 {
    const k = array.len(a);
    var gt: u32 = 0;
    var lt: u32 = 0;
    var i = 0;
    while (i < k) : (i += 1) {
        const x = a[i];
        const y = b[i];
        const differ = nz_mask(x ^ y);
        gt = (gt & ~differ) | (lt_mask(y, x) & differ);
        lt = (lt & ~differ) | (lt_mask(x, y) & differ);
    }
    return i64(gt & 1) - i64(lt & 1);
}

/// `out = a + b` over `k` limbs, answering with the carry out.
fn addn(out: []u32, a: []u32, b: []u32, k: i64) u32 {
    var carry: u64 = 0;
    var i = 0;
    while (i < k) : (i += 1) {
        const s = u64(a[i]) + u64(b[i]) + carry;
        out[i] = u32(s);
        carry = s >> 32;
    }
    return u32(carry);
}

/// `out = a - b` over `k` limbs, answering with the borrow out.
fn subn(out: []u32, a: []u32, b: []u32, k: i64) u32 {
    var borrow: u64 = 0;
    var i = 0;
    while (i < k) : (i += 1) {
        // Widened, so the subtraction cannot wrap and bit 63 is the borrow.
        const d = u64(a[i]) - u64(b[i]) - borrow;
        out[i] = u32(d);
        borrow = (d >> 63) & 1;
    }
    return u32(borrow);
}

/// `out = a + b`, answering with the carry out.
pub fn add(out: []u32, a: []u32, b: []u32) u32 {
    return addn(out, a, b, array.len(out));
}

/// `out = a - b`, answering with the borrow out.
pub fn sub(out: []u32, a: []u32, b: []u32) u32 {
    return subn(out, a, b, array.len(out));
}

/// `a -= n` when `take` is all ones, and nothing at all when it is all zeros.
fn cond_sub(a: []u32, n: []u32, take: u32, k: i64) void {
    var borrow: u64 = 0;
    var i = 0;
    while (i < k) : (i += 1) {
        const d = u64(a[i]) - u64(n[i] & take) - borrow;
        a[i] = u32(d);
        borrow = (d >> 63) & 1;
    }
    return;
}

// ---------------------------------------------------------------------------
// Montgomery arithmetic
// ---------------------------------------------------------------------------
//
// Montgomery form is what makes modular multiplication a multiplication and a
// shift rather than a division. A number `x` is represented as `x*R mod n`,
// where `R` is 2^(32*limbs) -- one past the top of the number -- and the
// product of two such is brought back into form by CIOS (Koc, Acar and
// Kaliski's coarsely integrated operand scanning), which interleaves the
// multiplication with the reduction so that nothing ever exceeds the width by
// more than two limbs.

pub const Mont = struct {
    /// The modulus, which must be odd.
    n: []u32,
    /// Its width, in limbs. A field rather than `array.len(n)` because the
    /// inner loops read it and a builtin call inside one is a stack walk
    /// under `--gc-stress`.
    limbs: i64,
    /// -n^-1 mod 2^32, which is what makes each CIOS step clear a limb.
    n0inv: u32,
    /// R^2 mod n, the multiplier that puts a number into Montgomery form.
    rr: []u32,
    /// The number one, for taking a number back out of it.
    one: []u32,
    /// The accumulator CIOS runs in, `limbs + 2` wide.
    t: []u32,
    /// A second scratch of the plain width, for the conditional subtracts.
    u: []u32,
};

/// -n0^-1 mod 2^32, by Newton's iteration.
///
/// `x = 1` is already the inverse modulo 2, because an odd number is its own
/// inverse there, and each step doubles the number of correct bits -- so five
/// steps take 1 bit to 32 and there is no sixth to write.
fn inv32(n0: u32) u32 {
    var x: u32 = 1;
    var i = 0;
    while (i < 5) : (i += 1) { x = x * (2 - n0 * x); }
    return 0 - x;
}

/// A Montgomery context for the odd modulus `n`.
///
/// `R^2 mod n` is computed by doubling one 64*limbs times rather than by
/// dividing, which is the whole reason this module needs no division. It costs
/// a few hundred thousand word operations for a 2048-bit modulus and happens
/// once per key.
pub fn mont(n: []u32) Mont {
    const k = array.len(n);
    // Both are mistakes in the caller rather than in its data: a caller that
    // has read a modulus off the wire checks it before it gets here.
    assert(k > 0);
    assert((n[0] & 1) == 1);

    const one = new(k);
    one[0] = 1;
    const rr = new(k);
    rr[0] = 1;
    const scratch = new(k);
    var i = 0;
    const doublings = 64 * k;
    while (i < doublings) : (i += 1) {
        const carry = addn(rr, rr, rr, k);
        const borrow = subn(scratch, rr, n, k);
        // Subtract once when the doubling did not fit, or when it fits and is
        // still at least the modulus. One subtract is always enough: the value
        // before it was under twice the modulus.
        cond_sub(rr, n, (0 - carry) | ~(0 - borrow), k);
    }

    return Mont{
        .n = n,
        .limbs = k,
        .n0inv = inv32(n[0]),
        .rr = rr,
        .one = one,
        .t = new(k + 2),
        .u = new(k),
    };
}

/// `out = a * b * R^-1 mod n`. `out` may alias either operand.
pub fn mont_mul(m: Mont, out: []u32, a: []u32, b: []u32) void {
    const k = m.limbs;
    const n = m.n;
    const t = m.t;
    const n0inv = m.n0inv;

    var i = 0;
    while (i < k + 2) : (i += 1) { t[i] = 0; }

    i = 0;
    while (i < k) : (i += 1) {
        // t += a * b[i]
        const bi = u64(b[i]);
        var carry: u64 = 0;
        var j = 0;
        while (j < k) : (j += 1) {
            const acc = u64(t[j]) + u64(a[j]) * bi + carry;
            t[j] = u32(acc);
            carry = acc >> 32;
        }
        var s = u64(t[k]) + carry;
        t[k] = u32(s);
        t[k + 1] = u32(s >> 32);

        // t += n * (t[0] * n0inv), which is chosen to make the low limb zero,
        // and then shift that zero limb off the bottom.
        const mi = u64(t[0] * n0inv);
        carry = 0;
        j = 0;
        while (j < k) : (j += 1) {
            const acc = u64(t[j]) + u64(n[j]) * mi + carry;
            t[j] = u32(acc);
            carry = acc >> 32;
        }
        s = u64(t[k]) + carry;
        t[k] = u32(s);
        t[k + 1] += u32(s >> 32);
        j = 0;
        while (j < k + 1) : (j += 1) { t[j] = t[j + 1]; }
        t[k + 1] = 0;
    }

    // What is left in `t[0..k]` is under twice the modulus, so at most one
    // subtract is needed. Which one to keep is decided with a mask, because
    // the answer depends on the operands and the operands are secret.
    var borrow: u64 = 0;
    var j = 0;
    while (j < k) : (j += 1) {
        const d = u64(t[j]) - u64(n[j]) - borrow;
        m.u[j] = u32(d);
        borrow = (d >> 63) & 1;
    }
    // `t[k]` holds the bit the subtraction has to borrow from. Take the
    // difference when there is one to borrow, or when there was no borrow.
    const take = (0 - t[k]) | ~(0 - u32(borrow));
    j = 0;
    while (j < k) : (j += 1) { out[j] = (m.u[j] & take) | (t[j] & ~take); }
    return;
}

/// `out = a * a * R^-1 mod n`.
pub fn mont_sqr(m: Mont, out: []u32, a: []u32) void {
    mont_mul(m, out, a, a);
    return;
}

/// `out = a + b mod n`, for `a` and `b` already reduced.
pub fn mont_add(m: Mont, out: []u32, a: []u32, b: []u32) void {
    const k = m.limbs;
    const n = m.n;
    const u = m.u;
    const carry = addn(u, a, b, k);
    var borrow: u64 = 0;
    var i = 0;
    while (i < k) : (i += 1) {
        const d = u64(u[i]) - u64(n[i]) - borrow;
        out[i] = u32(d);
        borrow = (d >> 63) & 1;
    }
    const take = (0 - carry) | ~(0 - u32(borrow));
    i = 0;
    while (i < k) : (i += 1) { out[i] = (out[i] & take) | (u[i] & ~take); }
    return;
}

/// `out = a - b mod n`, for `a` and `b` already reduced.
pub fn mont_sub(m: Mont, out: []u32, a: []u32, b: []u32) void {
    const k = m.limbs;
    const n = m.n;
    const borrow = subn(out, a, b, k);
    // Add the modulus back exactly when the difference went below zero.
    const take = 0 - borrow;
    var carry: u64 = 0;
    var i = 0;
    while (i < k) : (i += 1) {
        const s = u64(out[i]) + u64(n[i] & take) + carry;
        out[i] = u32(s);
        carry = s >> 32;
    }
    return;
}

/// `out = a * R mod n` -- into Montgomery form.
pub fn to_mont(m: Mont, out: []u32, a: []u32) void {
    mont_mul(m, out, a, m.rr);
    return;
}

/// `out = a * R^-1 mod n` -- out of Montgomery form.
pub fn from_mont(m: Mont, out: []u32, a: []u32) void {
    mont_mul(m, out, a, m.one);
    return;
}

/// `out = 1 * R mod n`, the Montgomery form of one.
pub fn mont_one(m: Mont, out: []u32) void {
    to_mont(m, out, m.one);
    return;
}

/// `out = base^e mod n`, square and multiply, most significant bit first.
///
/// **This one is deliberately not constant time**, and every caller it has is
/// a reason it need not be: an RSA public exponent is printed in the
/// certificate. A secret exponent would want a ladder and a fixed bit count,
/// and there is no secret exponent here because this module verifies
/// signatures and never makes them.
pub fn modexp(m: Mont, out: []u32, base: []u32, e: []u32) void {
    const k = m.limbs;
    const acc = new(k);
    const b = new(k);
    mont_one(m, acc);
    to_mont(m, b, base);
    var i = bit_len(e) - 1;
    while (i >= 0) {
        mont_mul(m, acc, acc, acc);
        if (bit(e, i) == 1) { mont_mul(m, acc, acc, b); }
        i -= 1;
    }
    from_mont(m, out, acc);
    return;
}
