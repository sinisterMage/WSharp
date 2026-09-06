// X25519, the Diffie-Hellman function on Curve25519 -- RFC 7748.
//
// Named for the curve rather than for the function because stage four's
// Ed25519 lives on the same field and will want everything below the ladder
// exactly as it is written here.
//
// **A field element is sixteen limbs of sixteen bits in `i64`s**, which is
// TweetNaCl's representation rather than ref10's ten limbs of twenty-five and
// a half. The wide limbs are faster and the narrow ones are far easier to be
// sure of: every partial product is at most 2^32, sixteen of them at most
// 2^36, and the fold that wraps 2^256 back down multiplies by 38 to reach
// about 2^41 -- twenty-two bits of headroom in an `i64` for a schoolbook
// multiplication anyone can check by reading it. This codebase has taken the
// auditable side of that trade twice already, in AES's computed S-box and in
// GHASH's 128 shifts, and for the same reason: nothing here is fast enough to
// matter and everything here is wrong in ways nothing reports.
//
// Signed limbs, because subtraction is then a subtraction. The carry pass
// biases by 2^16 and takes the difference back off, which is what makes an
// arithmetic shift the right thing to do to a limb that has gone negative.
//
// **Constant time is a construction, not a guarantee**, the same as in
// `std/cipher`: no index is computed from a secret, no loop leaves early on
// one, and the ladder's conditional swap is a mask. What W# cannot control is
// Cranelift, which may turn a branchless expression into a branch. This is a
// best effort against a local attacker rather than a promise.
//
// **Nothing allocates below `x25519`.** Every field routine writes into an
// output and a scratch the caller owns, because the ladder runs 255 times and
// the whole case suite runs a second time collecting at every allocation.
const array = @import("std/array");
const bytes = @import("std/bytes");

// ---------------------------------------------------------------------------
// The field, mod 2^255 - 19
// ---------------------------------------------------------------------------

/// 121665, the curve constant `(A - 2) / 4`, as a field element.
///
/// A top-level `const` array, so naming it costs an address rather than an
/// allocation -- which matters because the ladder names it once per bit.
const A24 = []i64{
    0xdb41, 1, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
};

/// A field element: sixteen limbs, zeroed.
fn fe() []i64 {
    const out: []i64 = array.new(16);
    return out;
}

/// `o = a`.
fn fe_copy(o: []i64, a: []i64) void {
    var i = 0;
    while (i < 16) : (i += 1) { o[i] = a[i]; }
    return;
}

/// `o = a + b`, limbwise and unreduced -- the carry pass inside `fe_mul` is
/// what brings the limbs back down, and it has room for several of these.
fn fe_add(o: []i64, a: []i64, b: []i64) void {
    var i = 0;
    while (i < 16) : (i += 1) { o[i] = a[i] + b[i]; }
    return;
}

/// `o = a - b`, likewise.
fn fe_sub(o: []i64, a: []i64, b: []i64) void {
    var i = 0;
    while (i < 16) : (i += 1) { o[i] = a[i] - b[i]; }
    return;
}

/// One carry pass over `o`, folding what leaves the top back into limb zero.
///
/// The `+ 65536` and the matching `c - 1` are what make this work for a
/// negative limb: an arithmetic shift of a negative number rounds towards
/// minus infinity, so the bias moves the value into the range where the shift
/// means what it is being asked to mean.
///
/// 2^256 is 38 mod (2^255 - 19), which is the factor limb fifteen's overflow
/// comes back in by.
fn fe_carry(o: []i64) void {
    var i = 0;
    while (i < 16) : (i += 1) {
        o[i] += 65536;
        const c = o[i] >> 16;
        if (i < 15) { o[i + 1] += c - 1; } else { o[0] += 38 * (c - 1); }
        o[i] -= c << 16;
    }
    return;
}

/// `o = a * b`, through the caller's 31-limb scratch `t`.
///
/// `o` may be either operand: nothing is written to it until the product is
/// finished. The scratch is the caller's for the reason everything here is --
/// the ladder calls this ten times a bit.
fn fe_mul(o: []i64, a: []i64, b: []i64, t: []i64) void {
    var i = 0;
    while (i < 31) : (i += 1) { t[i] = 0; }
    i = 0;
    while (i < 16) : (i += 1) {
        const ai = a[i];
        var j = 0;
        while (j < 16) : (j += 1) { t[i + j] += ai * b[j]; }
    }
    // Everything from limb sixteen up is worth 2^256 times its position, which
    // is 38 times its position in the bottom half.
    i = 0;
    while (i < 15) : (i += 1) { t[i] += 38 * t[i + 16]; }
    i = 0;
    while (i < 16) : (i += 1) { o[i] = t[i]; }
    fe_carry(o);
    fe_carry(o);
    return;
}

/// `o = a * a`.
fn fe_sqr(o: []i64, a: []i64, t: []i64) void {
    fe_mul(o, a, a, t);
    return;
}

/// Swap `p` and `q` when `b` is one, and do nothing when it is zero -- with a
/// mask rather than an `if`, because `b` is a bit of the secret scalar.
fn fe_swap(p: []i64, q: []i64, b: i64) void {
    const c = ~(b - 1);
    var i = 0;
    while (i < 16) : (i += 1) {
        const t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
    return;
}

/// `o = a^-1`, by `a^(p-2)` -- 254 squarings, skipping the two multiplies the
/// exponent's zero bits ask to be skipped.
///
/// `o` may be `a`. `c` and `t` are the caller's scratch.
fn fe_inv(o: []i64, a: []i64, c: []i64, t: []i64) void {
    fe_copy(c, a);
    var i = 253;
    while (i >= 0) {
        fe_sqr(c, c, t);
        if (i != 2 and i != 4) { fe_mul(c, c, a, t); }
        i -= 1;
    }
    fe_copy(o, c);
    return;
}

/// `n` as 32 little-endian bytes, fully reduced.
///
/// Three carry passes bring the limbs into range, and then the value is
/// reduced modulo the prime by *conditionally* subtracting it twice -- with
/// the same mask-swap the ladder uses, so a number that needed the subtraction
/// and one that did not take the same path.
fn fe_pack(o: []u8, n: []i64, t: []i64, m: []i64) void {
    fe_copy(t, n);
    fe_carry(t);
    fe_carry(t);
    fe_carry(t);
    var j = 0;
    while (j < 2) : (j += 1) {
        m[0] = t[0] - 0xffed;
        var i = 1;
        while (i < 15) : (i += 1) {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        const b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        fe_swap(t, m, 1 - b);
    }
    var i = 0;
    while (i < 16) : (i += 1) {
        o[2 * i] = u8(t[i] & 0xff);
        o[2 * i + 1] = u8((t[i] >> 8) & 0xff);
    }
    return;
}

/// 32 little-endian bytes as a field element.
///
/// Bit 255 is dropped rather than rejected, which RFC 7748 section 5 requires:
/// a peer is allowed to send a `u` whose top bit is set, and the receiver is
/// required to ignore it rather than to fail.
fn fe_unpack(o: []i64, n: []u8) void {
    var i = 0;
    while (i < 16) : (i += 1) {
        o[i] = i64(n[2 * i]) + (i64(n[2 * i + 1]) << 8);
    }
    o[15] &= 0x7fff;
    return;
}

// ---------------------------------------------------------------------------
// The ladder
// ---------------------------------------------------------------------------

/// Everything one scalar multiplication needs, allocated once.
///
/// This is the shape `Sha256` has and for the same reason: the ladder runs 255
/// times, and an allocation inside it would be 2,550 collections under
/// `--gc-stress`.
const Work = struct {
    a: []i64, b: []i64, c: []i64, d: []i64, e: []i64, f: []i64,
    x: []i64,
    /// `fe_mul`'s 31-limb product.
    t: []i64,
    /// `fe_inv`'s accumulator, and `fe_pack`'s two.
    iv: []i64, pt: []i64, pm: []i64,
};

fn work() Work {
    const t: []i64 = array.new(31);
    return Work{
        .a = fe(), .b = fe(), .c = fe(), .d = fe(), .e = fe(), .f = fe(),
        .x = fe(), .t = t, .iv = fe(), .pt = fe(), .pm = fe(),
    };
}

/// The Montgomery ladder of RFC 7748 section 5, on the `u` coordinate alone.
///
/// Every step does the same work whichever way the bit went: the two points
/// are swapped into place by a mask before the step and swapped back after it,
/// so nothing branches on the scalar.
fn ladder(out: []u8, s: []u8, u: []u8, w: Work) void {
    fe_unpack(w.x, u);
    var i = 0;
    while (i < 16) : (i += 1) {
        w.b[i] = w.x[i];
        w.a[i] = 0;
        w.c[i] = 0;
        w.d[i] = 0;
    }
    w.a[0] = 1;
    w.d[0] = 1;

    i = 254;
    while (i >= 0) {
        const r = i64((s[i >> 3] >> u8(i & 7)) & 1);
        fe_swap(w.a, w.b, r);
        fe_swap(w.c, w.d, r);
        fe_add(w.e, w.a, w.c);
        fe_sub(w.a, w.a, w.c);
        fe_add(w.c, w.b, w.d);
        fe_sub(w.b, w.b, w.d);
        fe_sqr(w.d, w.e, w.t);
        fe_sqr(w.f, w.a, w.t);
        fe_mul(w.a, w.c, w.a, w.t);
        fe_mul(w.c, w.b, w.e, w.t);
        fe_add(w.e, w.a, w.c);
        fe_sub(w.a, w.a, w.c);
        fe_sqr(w.b, w.a, w.t);
        fe_sub(w.c, w.d, w.f);
        fe_mul(w.a, w.c, A24, w.t);
        fe_add(w.a, w.a, w.d);
        fe_mul(w.c, w.c, w.a, w.t);
        fe_mul(w.a, w.d, w.f, w.t);
        fe_mul(w.d, w.b, w.x, w.t);
        fe_sqr(w.b, w.e, w.t);
        fe_swap(w.a, w.b, r);
        fe_swap(w.c, w.d, r);
        i -= 1;
    }

    // The answer is X/Z, and this is the one inversion the whole thing costs.
    fe_inv(w.c, w.c, w.iv, w.t);
    fe_mul(w.a, w.a, w.c, w.t);
    fe_pack(out, w.a, w.pt, w.pm);
    return;
}

/// The scalar with the bits RFC 7748 section 5 requires cleared and set.
///
/// A fresh copy, because the caller's key is not this function's to change.
fn clamped(scalar: []u8) []u8 {
    const s = bytes.new(32);
    bytes.copy(s, 0, scalar, 0, 32);
    s[0] &= 248;
    s[31] = (s[31] & 127) | 64;
    return s;
}

/// The X25519 function: `scalar` times the point with `u` coordinate `u`.
///
/// An all-zero result means the peer sent a point of small order, and RFC 8446
/// section 7.4.2 requires a TLS client to abort rather than to use it -- so it
/// is `error.WeakPoint` here rather than a value a caller could forget to
/// check.
pub fn x25519(scalar: []u8, u: []u8) ![]u8 {
    // A wrong length is a mistake in the caller: both are fixed-size keys.
    assert(array.len(scalar) == 32);
    assert(array.len(u) == 32);
    const out = bytes.new(32);
    ladder(out, clamped(scalar), u, work());
    var acc: u8 = 0;
    var i = 0;
    while (i < 32) : (i += 1) { acc |= out[i]; }
    if (acc == 0) { return error.WeakPoint; }
    return out;
}

/// `scalar` times the base point, whose `u` coordinate is 9.
///
/// This one cannot produce a small-order point -- the base point has prime
/// order and the clamping makes the scalar a nonzero multiple -- so it does
/// not raise.
pub fn x25519_base(scalar: []u8) []u8 {
    assert(array.len(scalar) == 32);
    const base = bytes.new(32);
    base[0] = 9;
    const out = bytes.new(32);
    ladder(out, clamped(scalar), base, work());
    return out;
}

/// One field multiplication over packed bytes, exposed so a test can assert
/// the field's own identities rather than only the curve's.
///
/// The same reason `aes_sub_byte` is exposed: a primitive whose correctness a
/// property test can state should be reachable from one.
pub fn field_mul(a: []u8, b: []u8) []u8 {
    assert(array.len(a) == 32);
    assert(array.len(b) == 32);
    const w = work();
    fe_unpack(w.a, a);
    fe_unpack(w.b, b);
    fe_mul(w.c, w.a, w.b, w.t);
    const out = bytes.new(32);
    fe_pack(out, w.c, w.pt, w.pm);
    return out;
}

/// The inverse of a field element, over packed bytes, exposed for the same
/// reason as `field_mul`.
pub fn field_inv(a: []u8) []u8 {
    assert(array.len(a) == 32);
    const w = work();
    fe_unpack(w.a, a);
    fe_inv(w.b, w.a, w.iv, w.t);
    const out = bytes.new(32);
    fe_pack(out, w.b, w.pt, w.pm);
    return out;
}
