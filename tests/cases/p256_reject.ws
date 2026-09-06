// The keys P-256 must refuse, each for its own reason.
//
// This curve has no small subgroup to fall into -- its cofactor is one -- so
// the failure X25519 guards against by checking its *output* cannot happen
// here. What can happen instead is the invalid-curve attack: a peer sends a
// point that is not on P-256 at all but is on some other curve with the same
// equation and a different b, whose group is smooth, and each exchange leaks
// the private key modulo a small factor. A handful of them recover it. So the
// check is on the *input*, and it is not optional -- RFC 8446 section 4.2.8.2
// says a receiver must make it.
//
// Each way of being wrong gets its own line, because they are five different
// checks and one test that merely says "a bad key is refused" would pass with
// four of them missing.
// expect: off the curve refused
// expect: x at the modulus refused
// expect: y at the modulus refused
// expect: a short key refused
// expect: a compressed key refused
// expect: the origin refused
// expect: a zero scalar refused
// expect: a scalar at the order refused
// expect: a scalar above the order refused
// expect: a real exchange is not refused
const p256 = @import("std/p256");
const bytes = @import("std/bytes");
const array = @import("std/array");
const text = @import("std/str");

const KEY = "c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433";
const P = "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff";
const GX = "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296";
const GY = "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5";
const ZERO = "0000000000000000000000000000000000000000000000000000000000000000";

fn main() i64 {
    const key = hex(KEY);
    const good = p256.derive(key) catch return 1;

    // A point one bit off the curve. This is the attack, and everything else
    // in this file is a way of getting to it through a shortcut.
    bad_point(flip(good, 64), "off the curve refused");
    // A coordinate at the modulus is a second spelling of zero -- the check is
    // "below p", not "fits in 32 bytes".
    bad_point(point(P, GY), "x at the modulus refused");
    bad_point(point(GX, P), "y at the modulus refused");
    bad_point(bytes.slice(good, 0, 64), "a short key refused");
    // 0x02 is a compressed point, which this group does not accept.
    bad_point(compressed(good), "a compressed key refused");
    // The point at infinity has no uncompressed encoding, so the nearest thing
    // to writing it down is (0, 0) -- which is simply not on the curve.
    bad_point(point(ZERO, ZERO), "the origin refused");

    // And the scalar has a range of its own: 1 up to the order, exclusive.
    bad_scalar(hex(ZERO), "a zero scalar refused");
    bad_scalar(hex("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551"),
               "a scalar at the order refused");
    bad_scalar(hex("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632552"),
               "a scalar above the order refused");

    // The other half of every claim above: the good key still works.
    const secret = p256.ecdh(key, good) catch return 2;
    if (array.len(secret) == 32) { print("a real exchange is not refused"); }
    return 0;
}

fn bad_point(peer: []u8, note: str) void {
    if (p256.valid(peer)) { return; }
    const out = p256.ecdh(hex(KEY), peer) catch |e| {
        if (e == error.BadPoint) { print(note); }
        bytes.new(0)
    };
    return;
}

fn bad_scalar(secret: []u8, note: str) void {
    const out = p256.derive(secret) catch |e| {
        if (e == error.BadScalar) { print(note); }
        bytes.new(0)
    };
    return;
}

fn flip(b: []u8, at: i64) []u8 {
    const out = bytes.slice(b, 0, array.len(b));
    out[at] ^= 1;
    return out;
}

fn compressed(b: []u8) []u8 {
    const out = bytes.slice(b, 0, 33);
    out[0] = 2;
    return out;
}

/// An uncompressed point from two written coordinates.
fn point(x: str, y: str) []u8 {
    return hex(text.concat(text.concat("04", x), y));
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
