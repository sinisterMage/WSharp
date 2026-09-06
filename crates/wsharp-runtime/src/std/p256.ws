// ECDH on NIST P-256 -- secp256r1, the group TLS 1.3 calls `secp256r1` and
// the one a server that will not do X25519 will do instead.
//
// The field is `std/bignum`'s Montgomery arithmetic at eight limbs, not a
// hand-written reduction for this prime. P-256's modulus is a Solinas prime
// and the fast way to reduce modulo it is a page of shifted additions with
// signed corrections, which is a page of security-critical code that exists
// only to be quicker -- and this is not quick enough anywhere for that to buy
// anything. One Montgomery multiplication, shared with RSA, is worth more than
// the difference.
//
// **Constant time is a construction, not a guarantee**, exactly as in
// `std/cipher` and `std/curve25519`: no index below is computed from a secret,
// no loop leaves early on one, and the ladder's conditional swap and the
// addition's exceptional cases are masks rather than branches.
//
// **Nothing allocates below `scalar_mul`.** Every field and point routine
// writes into storage the caller owns, and one `Work` is built per operation,
// because the ladder runs 256 times and the case suite runs a second time
// collecting at every allocation.
const array = @import("std/array");
const bytes = @import("std/bytes");
const bignum = @import("std/bignum");

// ---------------------------------------------------------------------------
// The curve
// ---------------------------------------------------------------------------
//
// y^2 = x^3 - 3x + b over F_p, with the base point G and the group order n.
// All five are `const` byte tables, so naming one costs an address.

/// p = 2^256 - 2^224 + 2^192 + 2^96 - 1.
const P = []u8{
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
};
/// The curve's b. Its a is -3, which is why the doubling below is the short one.
const B = []u8{
    0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7,
    0xb3, 0xeb, 0xbd, 0x55, 0x76, 0x98, 0x86, 0xbc,
    0x65, 0x1d, 0x06, 0xb0, 0xcc, 0x53, 0xb0, 0xf6,
    0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2, 0x60, 0x4b,
};
const GX = []u8{
    0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47,
    0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40, 0xf2,
    0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0,
    0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96,
};
const GY = []u8{
    0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b,
    0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e, 0x16,
    0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce,
    0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
};
/// The order of G. A private scalar must be in 1..n-1.
const N = []u8{
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84,
    0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
};

/// How many limbs a field element takes: 256 bits at 32 bits each.
const LIMBS = 8;

/// The curve's constants, in the form the arithmetic wants them.
pub const Curve = struct {
    /// Arithmetic modulo p.
    fp: bignum.Mont,
    /// One, in Montgomery form -- the multiplicative identity of the field and
    /// the x and y of the point at infinity.
    one: []u32,
    /// b, gx and gy, in Montgomery form.
    b: []u32,
    gx: []u32,
    gy: []u32,
    /// p - 2, the exponent an inversion is.
    pm2: []u32,
};

/// The curve, built once. Costs one Montgomery setup and a handful of arrays.
pub fn curve() Curve {
    const fp = bignum.mont(bignum.from_be(P, 0, 32, LIMBS));
    const one = bignum.new(LIMBS);
    bignum.mont_one(fp, one);
    const b = bignum.new(LIMBS);
    const gx = bignum.new(LIMBS);
    const gy = bignum.new(LIMBS);
    bignum.to_mont(fp, b, bignum.from_be(B, 0, 32, LIMBS));
    bignum.to_mont(fp, gx, bignum.from_be(GX, 0, 32, LIMBS));
    bignum.to_mont(fp, gy, bignum.from_be(GY, 0, 32, LIMBS));
    const two = bignum.new(LIMBS);
    two[0] = 2;
    const pm2 = bignum.new(LIMBS);
    bignum.sub(pm2, bignum.from_be(P, 0, 32, LIMBS), two);
    return Curve{ .fp = fp, .one = one, .b = b, .gx = gx, .gy = gy, .pm2 = pm2 };
}

// ---------------------------------------------------------------------------
// Points
// ---------------------------------------------------------------------------
//
// Jacobian coordinates: (X, Y, Z) stands for the affine (X/Z^2, Y/Z^3), which
// is what lets an addition and a doubling avoid the inversion an affine one
// would need. Z = 0 is the point at infinity, and its X and Y are one.

pub const Point = struct { x: []u32, y: []u32, z: []u32 };

fn point() Point {
    return Point{ .x = bignum.new(LIMBS), .y = bignum.new(LIMBS), .z = bignum.new(LIMBS) };
}

fn pt_copy(r: Point, p: Point) void {
    bignum.copy(r.x, p.x);
    bignum.copy(r.y, p.y);
    bignum.copy(r.z, p.z);
    return;
}

/// Everything one scalar multiplication needs, allocated once.
const Work = struct {
    /// The ladder's two accumulators and the point an addition lands in.
    r0: Point, r1: Point, t: Point,
    /// The field temporaries the two formulas run in.
    a: []u32, b: []u32, c: []u32, d: []u32, e: []u32, f: []u32,
    g: []u32, h: []u32, i: []u32, j: []u32, k: []u32, l: []u32,
    /// Where a result is assembled before it is written back, so that an
    /// output may be one of its own inputs.
    ox: []u32, oy: []u32, oz: []u32,
    /// The inversion's accumulator and base.
    ia: []u32, ib: []u32,
};

fn work() Work {
    return Work{
        .r0 = point(), .r1 = point(), .t = point(),
        .a = bignum.new(LIMBS), .b = bignum.new(LIMBS), .c = bignum.new(LIMBS),
        .d = bignum.new(LIMBS), .e = bignum.new(LIMBS), .f = bignum.new(LIMBS),
        .g = bignum.new(LIMBS), .h = bignum.new(LIMBS), .i = bignum.new(LIMBS),
        .j = bignum.new(LIMBS), .k = bignum.new(LIMBS), .l = bignum.new(LIMBS),
        .ox = bignum.new(LIMBS), .oy = bignum.new(LIMBS), .oz = bignum.new(LIMBS),
        .ia = bignum.new(LIMBS), .ib = bignum.new(LIMBS),
    };
}

/// `r = 2p`, by the doubling that a = -3 makes short (`dbl-2001-b`).
///
/// It needs no special case for the point at infinity: with Z = 0 the last
/// line gives Z3 = (Y + 0)^2 - Y^2 - 0 = 0, so infinity doubles to infinity
/// and the ladder can start there.
fn pt_double(c: Curve, r: Point, p: Point, w: Work) void {
    const m = c.fp;
    bignum.mont_sqr(m, w.a, p.z);             // delta = Z^2
    bignum.mont_sqr(m, w.b, p.y);             // gamma = Y^2
    bignum.mont_mul(m, w.c, p.x, w.b);        // beta = X*gamma
    bignum.mont_sub(m, w.d, p.x, w.a);        // X - delta
    bignum.mont_add(m, w.e, p.x, w.a);        // X + delta
    bignum.mont_mul(m, w.f, w.d, w.e);
    bignum.mont_add(m, w.g, w.f, w.f);
    bignum.mont_add(m, w.g, w.g, w.f);        // alpha = 3*(X-delta)*(X+delta)

    bignum.mont_add(m, w.h, w.c, w.c);        // 2*beta
    bignum.mont_add(m, w.i, w.h, w.h);        // 4*beta
    bignum.mont_add(m, w.j, w.i, w.i);        // 8*beta
    bignum.mont_sqr(m, w.k, w.g);
    bignum.mont_sub(m, w.ox, w.k, w.j);       // X3 = alpha^2 - 8*beta

    bignum.mont_add(m, w.l, p.y, p.z);
    bignum.mont_sqr(m, w.l, w.l);
    bignum.mont_sub(m, w.l, w.l, w.b);
    bignum.mont_sub(m, w.oz, w.l, w.a);       // Z3 = (Y+Z)^2 - gamma - delta

    bignum.mont_sub(m, w.d, w.i, w.ox);       // 4*beta - X3
    bignum.mont_mul(m, w.e, w.g, w.d);
    bignum.mont_sqr(m, w.f, w.b);             // gamma^2
    bignum.mont_add(m, w.f, w.f, w.f);
    bignum.mont_add(m, w.f, w.f, w.f);
    bignum.mont_add(m, w.f, w.f, w.f);        // 8*gamma^2
    bignum.mont_sub(m, w.oy, w.e, w.f);       // Y3 = alpha*(4*beta-X3) - 8*gamma^2

    bignum.copy(r.x, w.ox);
    bignum.copy(r.y, w.oy);
    bignum.copy(r.z, w.oz);
    return;
}

/// `r = p + q`, by `add-2007-bl`.
///
/// The formula is right for any two points that are neither equal nor at
/// infinity, and it happens to be right for `p == -q` as well -- H is then
/// zero, so Z3 is zero, which is the infinity that answer should be.
///
/// **`p == q` is the case it cannot do**, and the ladder is what rules it out:
/// its two accumulators satisfy `R1 - R0 = P` throughout, and `P` is never the
/// point at infinity because the caller has checked it. So the only cases left
/// are an operand *at* infinity -- `R0` starts there -- and those are settled
/// by selecting the other operand with a mask rather than by branching, since
/// which one it was is a fact about the scalar.
fn pt_add(c: Curve, r: Point, p: Point, q: Point, w: Work) void {
    const m = c.fp;
    bignum.mont_sqr(m, w.a, p.z);             // Z1Z1
    bignum.mont_sqr(m, w.b, q.z);             // Z2Z2
    bignum.mont_mul(m, w.c, p.x, w.b);        // U1 = X1*Z2Z2
    bignum.mont_mul(m, w.d, q.x, w.a);        // U2 = X2*Z1Z1
    bignum.mont_mul(m, w.e, p.y, q.z);
    bignum.mont_mul(m, w.e, w.e, w.b);        // S1 = Y1*Z2*Z2Z2
    bignum.mont_mul(m, w.f, q.y, p.z);
    bignum.mont_mul(m, w.f, w.f, w.a);        // S2 = Y2*Z1*Z1Z1

    bignum.mont_sub(m, w.g, w.d, w.c);        // H = U2 - U1
    bignum.mont_add(m, w.h, w.g, w.g);
    bignum.mont_sqr(m, w.h, w.h);             // I = (2H)^2
    bignum.mont_mul(m, w.i, w.g, w.h);        // J = H*I
    bignum.mont_sub(m, w.j, w.f, w.e);
    bignum.mont_add(m, w.j, w.j, w.j);        // r = 2*(S2 - S1)
    bignum.mont_mul(m, w.k, w.c, w.h);        // V = U1*I

    bignum.mont_sqr(m, w.l, w.j);
    bignum.mont_sub(m, w.l, w.l, w.i);
    bignum.mont_sub(m, w.ox, w.l, w.k);
    bignum.mont_sub(m, w.ox, w.ox, w.k);      // X3 = r^2 - J - 2V

    bignum.mont_sub(m, w.l, w.k, w.ox);
    bignum.mont_mul(m, w.l, w.j, w.l);
    bignum.mont_mul(m, w.d, w.e, w.i);
    bignum.mont_add(m, w.d, w.d, w.d);
    bignum.mont_sub(m, w.oy, w.l, w.d);       // Y3 = r*(V-X3) - 2*S1*J

    bignum.mont_add(m, w.l, p.z, q.z);
    bignum.mont_sqr(m, w.l, w.l);
    bignum.mont_sub(m, w.l, w.l, w.a);
    bignum.mont_sub(m, w.l, w.l, w.b);
    bignum.mont_mul(m, w.oz, w.l, w.g);       // Z3 = ((Z1+Z2)^2-Z1Z1-Z2Z2)*H

    // An operand at infinity is the other operand's answer. Both masks are
    // applied, in that order, so two infinities still give an infinity.
    const p_inf = bignum.zero_mask(p.z);
    const q_inf = bignum.zero_mask(q.z);
    bignum.select(w.ox, q.x, w.ox, p_inf);
    bignum.select(w.oy, q.y, w.oy, p_inf);
    bignum.select(w.oz, q.z, w.oz, p_inf);
    bignum.select(w.ox, p.x, w.ox, q_inf);
    bignum.select(w.oy, p.y, w.oy, q_inf);
    bignum.select(w.oz, p.z, w.oz, q_inf);

    bignum.copy(r.x, w.ox);
    bignum.copy(r.y, w.oy);
    bignum.copy(r.z, w.oz);
    return;
}

/// Swap `p` and `q` when `take` is all ones, with no branch.
fn pt_swap(p: Point, q: Point, take: u32, w: Work) void {
    bignum.select(w.ox, q.x, p.x, take);
    bignum.select(w.oy, q.y, p.y, take);
    bignum.select(w.oz, q.z, p.z, take);
    bignum.select(q.x, p.x, q.x, take);
    bignum.select(q.y, p.y, q.y, take);
    bignum.select(q.z, p.z, q.z, take);
    bignum.copy(p.x, w.ox);
    bignum.copy(p.y, w.oy);
    bignum.copy(p.z, w.oz);
    return;
}

/// `out = a^-1` in the field, by `a^(p-2)`.
///
/// The exponent is the modulus and so is public; only the base is secret, and
/// the base's value never decides what this does. That is what makes an
/// ordinary square-and-multiply constant time *here*.
fn fe_inv(c: Curve, out: []u32, a: []u32, w: Work) void {
    const m = c.fp;
    bignum.copy(w.ia, c.one);
    bignum.copy(w.ib, a);
    var i = 255;
    while (i >= 0) {
        bignum.mont_mul(m, w.ia, w.ia, w.ia);
        if (bignum.bit(c.pm2, i) == 1) { bignum.mont_mul(m, w.ia, w.ia, w.ib); }
        i -= 1;
    }
    bignum.copy(out, w.ia);
    return;
}

/// `r = k * p`, by the Montgomery ladder.
///
/// Every step does the same two operations whichever way the bit went: the
/// accumulators are swapped into place before it and back afterwards, both
/// with a mask. The invariant `R1 - R0 = p` holds throughout, which is the
/// argument `pt_add` relies on.
fn scalar_mul(c: Curve, r: Point, k: []u8, p: Point, w: Work) void {
    // R0 = infinity, R1 = p.
    bignum.copy(w.r0.x, c.one);
    bignum.copy(w.r0.y, c.one);
    var i = 0;
    while (i < LIMBS) : (i += 1) { w.r0.z[i] = 0; }
    pt_copy(w.r1, p);

    i = 255;
    while (i >= 0) {
        const take = 0 - u32((k[31 - (i / 8)] >> u8(i % 8)) & 1);
        pt_swap(w.r0, w.r1, take, w);
        pt_add(c, w.t, w.r0, w.r1, w);
        pt_double(c, w.r0, w.r0, w);
        pt_copy(w.r1, w.t);
        pt_swap(w.r0, w.r1, take, w);
        i -= 1;
    }
    pt_copy(r, w.r0);
    return;
}

/// The affine coordinates of `p`, each as 32 big-endian bytes, written into
/// `out` at `at`.
///
/// The point at infinity has no affine form, and both callers have an argument
/// that they cannot hand one over -- a scalar in 1..n-1 against a group of
/// prime order n has no multiple that is the identity. The check is here
/// anyway, in the one place that would otherwise write out `(0, 0)` and call
/// it a public key, because that is the failure worth a line.
fn pt_affine(c: Curve, out: []u8, at: i64, p: Point, w: Work) !void {
    if (bignum.is_zero(p.z)) { return error.BadPoint; }
    const m = c.fp;
    fe_inv(c, w.a, p.z, w);                   // 1/Z
    bignum.mont_sqr(m, w.b, w.a);             // 1/Z^2
    bignum.mont_mul(m, w.c, w.b, w.a);        // 1/Z^3
    bignum.mont_mul(m, w.d, p.x, w.b);
    bignum.mont_mul(m, w.e, p.y, w.c);
    bignum.from_mont(m, w.f, w.d);
    bignum.to_be_at(w.f, out, at, 32);
    bignum.from_mont(m, w.f, w.e);
    bignum.to_be_at(w.f, out, at + 32, 32);
    return;
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// All ones when `x < y`, for values that fit a `u32`.
fn lt_mask(x: u32, y: u32) u32 { return 0 - u32((u64(x) - u64(y)) >> 63); }

/// All ones when `x` is not zero.
fn nz_mask(x: u32) u32 { return lt_mask(0, x); }

/// True when the 32 bytes of `a` at `at`, read big-endian, are below `b`.
///
/// Every byte is examined: the first difference decides, and the rest are
/// looked at anyway, because where a key stops matching a bound is a fact
/// about the key.
fn below(a: []u8, at: i64, b: []u8) bool {
    var lt: u32 = 0;
    var settled: u32 = 0;
    var i = 0;
    while (i < 32) : (i += 1) {
        const x = u32(a[at + i]);
        const y = u32(b[i]);
        const differ = nz_mask(x ^ y);
        lt |= ~settled & differ & lt_mask(x, y);
        settled |= differ;
    }
    return (lt & 1) == 1;
}

/// True when `p` is a well-formed, uncompressed point on the curve.
///
/// Every part of this is required rather than defensive. RFC 8446 section
/// 4.2.8.2 says a peer's `key_share` for this group is an uncompressed point
/// and that a receiver must verify it: a coordinate at or above p is a second
/// encoding of a value, and a point off the curve is the invalid-curve attack,
/// which recovers a private key from a handful of exchanges.
pub fn valid(p: []u8) bool { return valid_with(curve(), work(), p); }

/// The same check, against a curve and a scratch the caller already has.
fn valid_with(c: Curve, w: Work, p: []u8) bool {
    if (array.len(p) != 65) { return false; }
    if (p[0] != 4) { return false; }
    if (!below(p, 1, P) or !below(p, 33, P)) { return false; }
    const m = c.fp;
    bignum.to_mont(m, w.a, bignum.from_be(p, 1, 32, LIMBS));    // x
    bignum.to_mont(m, w.b, bignum.from_be(p, 33, 32, LIMBS));   // y
    bignum.mont_sqr(m, w.c, w.b);                               // y^2
    bignum.mont_sqr(m, w.d, w.a);
    bignum.mont_mul(m, w.d, w.d, w.a);                          // x^3
    bignum.mont_add(m, w.e, w.a, w.a);
    bignum.mont_add(m, w.e, w.e, w.a);                          // 3x
    bignum.mont_sub(m, w.d, w.d, w.e);
    bignum.mont_add(m, w.d, w.d, c.b);                          // x^3 - 3x + b
    bignum.mont_sub(m, w.f, w.c, w.d);
    // The point at infinity has no uncompressed encoding, and (0, 0) is not on
    // the curve, so being on it is the whole of the check.
    return bignum.is_zero(w.f);
}

/// The scalar as limbs, or an error when it is not a private key.
///
/// Zero is not one, and neither is anything from n upwards: both would be a
/// key whose public point is the identity or a repeat of another key's.
fn private_scalar(secret: []u8) ![]u8 {
    if (array.len(secret) != 32) { return error.BadScalar; }
    if (!below(secret, 0, N)) { return error.BadScalar; }
    const k = bignum.from_be(secret, 0, 32, LIMBS);
    if (bignum.is_zero(k)) { return error.BadScalar; }
    return secret;
}

/// The public point for `secret`: 65 bytes, `0x04` then X then Y.
pub fn derive(secret: []u8) ![]u8 {
    const k = try private_scalar(secret);
    const c = curve();
    const w = work();
    const g = point();
    bignum.copy(g.x, c.gx);
    bignum.copy(g.y, c.gy);
    bignum.copy(g.z, c.one);
    const r = point();
    scalar_mul(c, r, k, g, w);
    const out = bytes.new(65);
    out[0] = 4;
    try pt_affine(c, out, 1, r, w);
    return out;
}

/// The ECDH shared secret: the X coordinate of `secret` times `peer`, as 32
/// big-endian bytes, which is what SEC1 section 3.3.1 says the secret is.
///
/// The Y coordinate is thrown away rather than never computed. Deriving both
/// is what the ladder does anyway, and a key-agreement function that answered
/// with a point would be one every caller had to remember to cut down.
pub fn ecdh(secret: []u8, peer: []u8) ![]u8 {
    const k = try private_scalar(secret);
    const c = curve();
    const w = work();
    if (!valid_with(c, w, peer)) { return error.BadPoint; }
    const q = point();
    bignum.to_mont(c.fp, q.x, bignum.from_be(peer, 1, 32, LIMBS));
    bignum.to_mont(c.fp, q.y, bignum.from_be(peer, 33, 32, LIMBS));
    bignum.copy(q.z, c.one);
    const r = point();
    scalar_mul(c, r, k, q, w);
    // A cofactor of one and a peer point already checked to be on the curve
    // leave only one way to reach infinity, and that is a scalar that is a
    // multiple of n -- which `private_scalar` has already refused. `pt_affine`
    // says so once more, because it is the place that would otherwise answer.
    const full = bytes.new(64);
    try pt_affine(c, full, 0, r, w);
    return bytes.slice(full, 0, 32);
}
