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
const hash = @import("std/hash");

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

// ---------------------------------------------------------------------------
// Ed25519
// ---------------------------------------------------------------------------
//
// The signature scheme on the twisted Edwards curve birationally equivalent to
// the Montgomery curve above -- which is why it is here rather than in a
// module of its own: every field operation it needs is already written, and
// writing them twice is how two copies come to disagree.
//
// The shape is TweetNaCl's, for the reason the field is: extended coordinates
// and a bit-by-bit ladder are slower than a windowed comb over precomputed
// multiples of the base point, and they are also short enough to read. The
// ladder does one addition and one doubling per bit whichever way the bit
// went, with the points swapped into place by a mask, so nothing branches on a
// secret.
//
// **Signing is here and RSA signing is not**, which is worth saying because
// the roadmap made a point of the second. RSA signing would need a
// constant-time `modexp` and the Chinese remainder theorem and has no caller;
// this has one, `std/tls`'s server, and it is the only signature this library
// can produce at all.
//
// **Verification is cofactorless**, the equation `[S]B = R + [h]A` rather than
// the eightfold one, with `S < L` and canonical encodings required. That is
// what RFC 8032 section 5.1.7 describes first and what OpenSSL, BoringSSL and
// Go all do, so a signature this accepts is one a TLS peer accepts.
// `ed25519_reject.ws` settles each refusal against a second implementation
// rather than against this comment.

/// `d = -121665/121666`, and `2d`, the twisted Edwards curve constants.
const D = []i64{
    0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070,
    0xe898, 0x7779, 0x4079, 0x8cc7, 0xfe73, 0x2b6f, 0x6cee, 0x5203,
};

const D2 = []i64{
    0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0,
    0xd130, 0xeef3, 0x80f2, 0x198e, 0xfce7, 0x56df, 0xd9dc, 0x2406,
};

/// The base point, whose `y` is 4/5 and whose `x` is the even root.
const BX = []i64{
    0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c,
    0xdc5c, 0xfdd6, 0xe231, 0xc0a4, 0x53fe, 0xcd6e, 0x36d3, 0x2169,
};

const BY = []i64{
    0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
    0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
};

/// `sqrt(-1)`, which decompression needs when the first root it tries is the
/// wrong one.
const SQRTM1 = []i64{
    0xa0b0, 0x4a0e, 0x1b27, 0xc4ee, 0xe478, 0xad2f, 0x1806, 0x2f43,
    0xd7a7, 0x3dfb, 0x0099, 0x2b4d, 0xdf0b, 0x4fc1, 0x2480, 0x2b83,
};

/// `L`, the order of the base point, little-endian.
const L = []i64{
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58,
    0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
};

/// `2^255 - 19`, little-endian, for the canonicality checks.
const P_LE = []i64{
    0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
};

// ---------------------------------------------------------------------------
// Two more field operations
// ---------------------------------------------------------------------------

fn fe_zero(o: []i64) void {
    var i = 0;
    while (i < 16) : (i += 1) { o[i] = 0; }
    return;
}

fn fe_one(o: []i64) void {
    fe_zero(o);
    o[0] = 1;
    return;
}

/// `o = a^((p-5)/8)`, the exponent a square root modulo this prime is built
/// from -- 251 squarings, skipping the one multiply the exponent's single zero
/// bit asks to be skipped. `fe_inv`'s shape with a different exponent.
fn fe_pow2523(o: []i64, a: []i64, c: []i64, t: []i64) void {
    fe_copy(c, a);
    var i = 250;
    while (i >= 0) {
        fe_sqr(c, c, t);
        if (i != 1) { fe_mul(c, c, a, t); }
        i -= 1;
    }
    fe_copy(o, c);
    return;
}

// ---------------------------------------------------------------------------
// Points
// ---------------------------------------------------------------------------

/// A point in extended coordinates: `x/z`, `y/z`, with `t = xy/z`.
///
/// The fourth coordinate is what makes the addition below complete -- one
/// formula for every pair of points, with no case for a doubling and none for
/// the identity, which is what a ladder over a secret scalar needs.
const Ge = struct { x: []i64, y: []i64, z: []i64, t: []i64 };

fn ge() Ge { return Ge{ .x = fe(), .y = fe(), .z = fe(), .t = fe() }; }

/// Everything one signature costs, allocated once.
///
/// Larger than `Work` above because there are more moving parts, and flat for
/// the same reason: the ladder runs 256 times over nine field multiplications,
/// so anything allocated below here would be a collection per step under
/// `--gc-stress`.
const Ed = struct {
    p: Ge, q: Ge, bp: Ge,
    /// `ge_add`'s eight, and one more for its two operand sums.
    a: []i64, b: []i64, c: []i64, d: []i64,
    e: []i64, f: []i64, g: []i64, h: []i64, s: []i64,
    /// Decompression's.
    num: []i64, den: []i64, den2: []i64, den4: []i64, den6: []i64,
    chk: []i64, tmp: []i64,
    /// `ge_pack`'s three.
    zi: []i64, tx: []i64, ty: []i64,
    /// `fe_inv` and `fe_pow2523`'s accumulator, `fe_pack`'s two, `fe_mul`'s
    /// 31-limb product.
    iv: []i64, pk1: []i64, pk2: []i64, t: []i64,
    /// Two packings, so two field elements can be compared as bytes.
    c1: []u8, c2: []u8,
    /// `modL`'s 64 accumulating digits.
    x: []i64,
};

fn ed_work() Ed {
    const t: []i64 = array.new(31);
    const x: []i64 = array.new(64);
    return Ed{
        .p = ge(), .q = ge(), .bp = ge(),
        .a = fe(), .b = fe(), .c = fe(), .d = fe(),
        .e = fe(), .f = fe(), .g = fe(), .h = fe(), .s = fe(),
        .num = fe(), .den = fe(), .den2 = fe(), .den4 = fe(), .den6 = fe(),
        .chk = fe(), .tmp = fe(),
        .zi = fe(), .tx = fe(), .ty = fe(),
        .iv = fe(), .pk1 = fe(), .pk2 = fe(), .t = t,
        .c1 = bytes.new(32), .c2 = bytes.new(32),
        .x = x,
    };
}

/// Whether two field elements are the same number, which is a question about
/// their reduced forms rather than their limbs.
fn fe_eq(a: []i64, b: []i64, w: Ed) bool {
    fe_pack(w.c1, a, w.pk1, w.pk2);
    fe_pack(w.c2, b, w.pk1, w.pk2);
    return bytes.equal(w.c1, w.c2);
}

/// The low bit of a field element's reduced form, which is the sign bit an
/// Edwards point is compressed with.
fn fe_par(a: []i64, w: Ed) i64 {
    fe_pack(w.c1, a, w.pk1, w.pk2);
    return i64(w.c1[0] & 1);
}

/// `p += q`, by the unified addition of Hisil, Wong, Carter and Dawson.
///
/// Every read of `q` happens before any write to `p`, which is what makes
/// `ge_add(p, p, w)` a doubling rather than a corruption -- and the ladder
/// below relies on exactly that.
fn ge_add(p: Ge, q: Ge, w: Ed) void {
    fe_sub(w.a, p.y, p.x);
    fe_sub(w.s, q.y, q.x);
    fe_mul(w.a, w.a, w.s, w.t);
    fe_add(w.b, p.x, p.y);
    fe_add(w.s, q.x, q.y);
    fe_mul(w.b, w.b, w.s, w.t);
    fe_mul(w.c, p.t, q.t, w.t);
    fe_mul(w.c, w.c, D2, w.t);
    fe_mul(w.d, p.z, q.z, w.t);
    fe_add(w.d, w.d, w.d);
    fe_sub(w.e, w.b, w.a);
    fe_sub(w.f, w.d, w.c);
    fe_add(w.g, w.d, w.c);
    fe_add(w.h, w.b, w.a);
    fe_mul(p.x, w.e, w.f, w.t);
    fe_mul(p.y, w.h, w.g, w.t);
    fe_mul(p.z, w.g, w.f, w.t);
    fe_mul(p.t, w.e, w.h, w.t);
    return;
}

fn ge_swap(p: Ge, q: Ge, b: i64) void {
    fe_swap(p.x, q.x, b);
    fe_swap(p.y, q.y, b);
    fe_swap(p.z, q.z, b);
    fe_swap(p.t, q.t, b);
    return;
}

/// `p = s * q`, left to right, one addition and one doubling per bit.
///
/// `p` and `q` must be different points. The swap before and after each step
/// is what keeps the work identical whichever way the bit went, exactly as the
/// Montgomery ladder above does it.
fn ge_scalarmult(p: Ge, q: Ge, s: []u8, w: Ed) void {
    fe_zero(p.x);
    fe_one(p.y);
    fe_one(p.z);
    fe_zero(p.t);
    var i = 255;
    while (i >= 0) {
        const b = i64((s[i >> 3] >> u8(i & 7)) & 1);
        ge_swap(p, q, b);
        ge_add(q, p, w);
        ge_add(p, p, w);
        ge_swap(p, q, b);
        i -= 1;
    }
    return;
}

fn ge_scalarbase(p: Ge, s: []u8, w: Ed) void {
    fe_copy(w.bp.x, BX);
    fe_copy(w.bp.y, BY);
    fe_one(w.bp.z);
    fe_mul(w.bp.t, BX, BY, w.t);
    ge_scalarmult(p, w.bp, s, w);
    return;
}

/// `out[0..32] = p`, compressed: `y` in 255 bits with `x`'s low bit on top.
fn ge_pack(out: []u8, p: Ge, w: Ed) void {
    fe_inv(w.zi, p.z, w.iv, w.t);
    fe_mul(w.tx, p.x, w.zi, w.t);
    fe_mul(w.ty, p.y, w.zi, w.t);
    fe_pack(out, w.ty, w.pk1, w.pk2);
    out[31] ^= u8(fe_par(w.tx, w) << 7);
    return;
}

/// `p = -A`, from a compressed encoding; false if it is not a point.
///
/// The negative rather than the point itself, because verification wants
/// `S*B - h*A` and this is one negation instead of one per ladder step. The
/// `x` recovered is `sqrt((y^2 - 1) / (d*y^2 + 1))`, which needs the fourth
/// root the prime allows and then a correction by `sqrt(-1)` when the exponent
/// gave the other root.
fn ge_unpack_neg(p: Ge, enc: []u8, w: Ed) bool {
    fe_one(p.z);
    fe_unpack(p.y, enc);
    fe_sqr(w.num, p.y, w.t);
    fe_mul(w.den, w.num, D, w.t);
    fe_sub(w.num, w.num, p.z);
    fe_add(w.den, p.z, w.den);

    fe_sqr(w.den2, w.den, w.t);
    fe_sqr(w.den4, w.den2, w.t);
    fe_mul(w.den6, w.den4, w.den2, w.t);
    fe_mul(w.tmp, w.den6, w.num, w.t);
    fe_mul(w.tmp, w.tmp, w.den, w.t);

    fe_pow2523(w.tmp, w.tmp, w.iv, w.t);
    fe_mul(w.tmp, w.tmp, w.num, w.t);
    fe_mul(w.tmp, w.tmp, w.den, w.t);
    fe_mul(w.tmp, w.tmp, w.den, w.t);
    fe_mul(p.x, w.tmp, w.den, w.t);

    fe_sqr(w.chk, p.x, w.t);
    fe_mul(w.chk, w.chk, w.den, w.t);
    if (!fe_eq(w.chk, w.num, w)) { fe_mul(p.x, p.x, SQRTM1, w.t); }

    fe_sqr(w.chk, p.x, w.t);
    fe_mul(w.chk, w.chk, w.den, w.t);
    // No square root at all: `y` is not the ordinate of any point.
    if (!fe_eq(w.chk, w.num, w)) { return false; }

    // The encoding says which root was meant; take the other one, since this
    // is answering with the negative.
    if (fe_par(p.x, w) == i64(enc[31] >> 7)) {
        fe_zero(w.tmp);
        fe_sub(p.x, w.tmp, p.x);
    }
    fe_mul(p.t, p.x, p.y, w.t);
    return true;
}

// ---------------------------------------------------------------------------
// Arithmetic modulo the group order
// ---------------------------------------------------------------------------
//
// A different modulus from the field's and a different shape of code: `L` is
// not a prime of convenient form, so this is Barrett-ish reduction over
// eight-bit digits, taken from TweetNaCl. The digits are `i64` and go
// negative in the middle, which is the same trick `fe_carry` above plays.

fn sc_mod_l(r: []u8, x: []i64) void {
    var carry = 0;
    var j = 0;
    var i = 63;
    while (i >= 32) {
        carry = 0;
        j = i - 32;
        while (j < i - 12) : (j += 1) {
            x[j] += carry - 16 * x[i] * L[j - (i - 32)];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry * 256;
        }
        x[j] += carry;
        x[i] = 0;
        i -= 1;
    }
    carry = 0;
    j = 0;
    while (j < 32) : (j += 1) {
        x[j] += carry - (x[31] >> 4) * L[j];
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    j = 0;
    while (j < 32) : (j += 1) { x[j] -= carry * L[j]; }
    i = 0;
    while (i < 32) : (i += 1) {
        x[i + 1] += x[i] >> 8;
        r[i] = u8(x[i] & 255);
    }
    return;
}

/// `out = wide mod L`, where `wide` is 64 bytes of hash.
fn sc_reduce(out: []u8, wide: []u8, x: []i64) void {
    var i = 0;
    while (i < 64) : (i += 1) { x[i] = i64(wide[i]); }
    sc_mod_l(out, x);
    return;
}

/// `out = (r + h * a) mod L`, the one place a signature needs a product.
fn sc_muladd(out: []u8, h: []u8, a: []u8, r: []u8, x: []i64) void {
    var i = 0;
    while (i < 64) : (i += 1) { x[i] = 0; }
    i = 0;
    while (i < 32) : (i += 1) { x[i] = i64(r[i]); }
    i = 0;
    while (i < 32) : (i += 1) {
        const hi = i64(h[i]);
        var j = 0;
        while (j < 32) : (j += 1) { x[i + j] += hi * i64(a[j]); }
    }
    sc_mod_l(out, x);
    return;
}

/// Whether 32 little-endian bytes are strictly below `bound`.
///
/// Variable time, and that is correct here: both callers are checking
/// something a peer sent, which is public by the time it has arrived.
fn below(enc: []u8, at: i64, bound: []i64, mask_top: bool) bool {
    var i = 31;
    while (i >= 0) {
        var v = i64(enc[at + i]);
        if (i == 31 and mask_top) { v = v & 0x7f; }
        if (v != bound[i]) { return v < bound[i]; }
        i -= 1;
    }
    return false;
}

// ---------------------------------------------------------------------------
// The signature scheme
// ---------------------------------------------------------------------------

/// The public key a 32-byte seed belongs to.
pub fn ed25519_public(seed: []u8) []u8 {
    assert(array.len(seed) == 32);
    const d = hash.sha512(seed);
    d[0] &= 248;
    d[31] &= 127;
    d[31] |= 64;
    const w = ed_work();
    ge_scalarbase(w.p, d, w);
    const out = bytes.new(32);
    ge_pack(out, w.p, w);
    return out;
}

/// A 64-byte signature over `msg`, by the key `seed` belongs to.
///
/// Deterministic: the nonce is a hash of the message under the half of the
/// seed's expansion that is not the scalar, which is what RFC 8032 specifies
/// and what makes a signature reproducible without a generator.
///
/// The public key is recomputed rather than taken as an argument, which costs
/// a second scalar multiplication. That is the honest interface -- a caller
/// holding a mismatched pair would otherwise sign with one key and claim the
/// other -- and a signature is not on any hot path here.
pub fn ed25519_sign(seed: []u8, msg: []u8) []u8 {
    assert(array.len(seed) == 32);
    const n = array.len(msg);
    const d = hash.sha512(seed);
    d[0] &= 248;
    d[31] &= 127;
    d[31] |= 64;
    const a = bytes.slice(d, 0, 32);

    // r = H(prefix || msg) mod L
    const rs = hash.sha512_init();
    hash.sha512_update(rs, d, 32, 32);
    hash.sha512_update(rs, msg, 0, n);
    const w = ed_work();
    const r = bytes.new(32);
    sc_reduce(r, hash.sha512_final(rs), w.x);

    // R = r * B
    const sig = bytes.new(64);
    ge_scalarbase(w.p, r, w);
    ge_pack(sig, w.p, w);

    // S = (r + H(R || A || msg) * a) mod L
    const pk = ed25519_public(seed);
    const hs = hash.sha512_init();
    hash.sha512_update(hs, sig, 0, 32);
    hash.sha512_update(hs, pk, 0, 32);
    hash.sha512_update(hs, msg, 0, n);
    const k = bytes.new(32);
    sc_reduce(k, hash.sha512_final(hs), w.x);
    const s = bytes.new(32);
    sc_muladd(s, k, a, r, w.x);
    bytes.copy(sig, 32, s, 0, 32);
    return sig;
}

/// Whether `sig` is a signature over `msg` by `pk`.
///
/// `bool` rather than `!void`, because there is exactly one thing a caller can
/// do about a refusal and no information in *which* refusal it was -- a
/// signature that fails a canonicality check and one that fails the equation
/// are equally not signatures. `std/tls` turns the `false` into its own error
/// at the point where the reason is known.
pub fn ed25519_verify(pk: []u8, msg: []u8, sig: []u8) bool {
    if (array.len(pk) != 32) { return false; }
    if (array.len(sig) != 64) { return false; }
    // S must be reduced. Without this a signature has many spellings, and a
    // protocol that hashes one is a protocol two peers disagree about.
    if (!below(sig, 32, L, false)) { return false; }
    // Both compressed points must be canonically encoded, for the same reason.
    if (!below(pk, 0, P_LE, true)) { return false; }
    if (!below(sig, 0, P_LE, true)) { return false; }

    const w = ed_work();
    if (!ge_unpack_neg(w.q, pk, w)) { return false; }

    const hs = hash.sha512_init();
    hash.sha512_update(hs, sig, 0, 32);
    hash.sha512_update(hs, pk, 0, 32);
    hash.sha512_update(hs, msg, 0, array.len(msg));
    const k = bytes.new(32);
    sc_reduce(k, hash.sha512_final(hs), w.x);

    // p = -h*A, then p += S*B, so p is S*B - h*A and should be R.
    ge_scalarmult(w.p, w.q, k, w);
    ge_scalarbase(w.q, bytes.slice(sig, 32, 64), w);
    ge_add(w.p, w.q, w);
    const t = bytes.new(32);
    ge_pack(t, w.p, w);
    return bytes.equal(t, bytes.slice(sig, 0, 32));
}
