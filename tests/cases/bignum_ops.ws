// Fixed-width numbers and Montgomery arithmetic, at the level RSA and P-256
// stand on.
//
// The reason this module exists in a language with no 128-bit type is one
// line of arithmetic, and it is the first thing checked below: a limb is 32
// bits, so `t + a*b + carry` is at most `(2^32-1)^2 + 2*(2^32-1)`, which is
// exactly `2^64 - 1`. Every product and every carry here fits a `u64` with no
// room to spare and none needed, which is why `bits.mulhi` -- the 64x64 -> 128
// product item 9 reserved a place for -- is still not written.
//
// The expected values were computed independently with arbitrary-precision
// integers, which is the one kind of second implementation that is not a
// second implementation of the same idea.
// expect: 0123456789abcdeffedcba98765432100f1e2d3c4b5a69788796a5b4c3d2e1f0
// expect: 8796a5b4c3d2e1f0
// expect: 00000000000000000123456789abcdeffedcba98765432100f1e2d3c4b5a69788796a5b4c3d2e1f0
// expect: 249
// expect: wrapping adds carry
// expect: borrowing subtracts borrow
// expect: ordered
// expect: montgomery round trips
// expect: 00000000fffffffeffffffffffffffffffffffff000000000000000000000001
// expect: 2882bd843cf8c8f4c3a0600362187669a938dc7bf79c56985fe3276eb4a565ba
// expect: 0123456789abcdeffedcba98765432100f1e2d3c4b5a69788796a5b4c3d2e1f0
// expect: modular addition wraps at the modulus
// expect: masks select
const bignum = @import("std/bignum");
const bytes = @import("std/bytes");

/// P-256's prime, borrowed for its shape rather than for its curve: it is odd,
/// it fills its width, and it has long runs of both bits.
const P = "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff";
const X = "0123456789abcdeffedcba98765432100f1e2d3c4b5a69788796a5b4c3d2e1f0";

fn main() i64 {
    const x = bignum.from_be(hex(X), 0, 32, 8);
    // A number is its bytes again, and `to_be` fills whatever it is given:
    // narrower truncates from the top, wider pads with zeros. Both matter,
    // because a DER integer and a fixed-width coordinate disagree about which
    // one they want.
    show(x, 32);
    show(x, 8);
    show(x, 40);
    print_int(bignum.bit_len(x));

    // The carry and the borrow are the return values, because the width is
    // fixed and there is nowhere else for them to go.
    const ones = bignum.from_be(hex("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"), 0, 32, 8);
    const one = bignum.new(8);
    one[0] = 1;
    const sum = bignum.new(8);
    if (bignum.add(sum, ones, one) == 1 and bignum.is_zero(sum)) {
        print("wrapping adds carry");
    }
    const diff = bignum.new(8);
    if (bignum.sub(diff, bignum.new(8), one) == 1 and bignum.cmp(diff, ones) == 0) {
        print("borrowing subtracts borrow");
    }
    if (bignum.cmp(x, ones) == -1 and bignum.cmp(ones, x) == 1 and bignum.cmp(x, x) == 0) {
        print("ordered");
    }

    const m = bignum.mont(bignum.from_be(hex(P), 0, 32, 8));
    const t = bignum.new(8);
    const back = bignum.new(8);
    bignum.to_mont(m, t, x);
    bignum.from_mont(m, back, t);
    if (bignum.cmp(back, x) == 0) { print("montgomery round trips"); }

    // 2^256 mod p is the one value that says the reduction is this prime's:
    // it is p subtracted from a number one bit wider than p.
    const two = bignum.new(8);
    two[0] = 2;
    const e256 = bignum.new(8);
    e256[0] = 256;
    const out = bignum.new(8);
    bignum.modexp(m, out, two, e256);
    show(out, 32);
    // The exponent an RSA verification actually uses.
    const e = bignum.new(8);
    e[0] = 65537;
    bignum.modexp(m, out, x, e);
    show(out, 32);
    // And the identity, which catches a square-and-multiply that has started
    // its accumulator at the wrong place.
    bignum.modexp(m, out, x, one);
    show(out, 32);

    // Modular addition and subtraction, at the seam.
    const pm1 = bignum.new(8);
    bignum.sub(pm1, m.n, one);
    bignum.mont_add(m, out, pm1, two);
    bignum.mont_sub(m, back, one, two);
    if (bignum.cmp(out, one) == 0 and bignum.cmp(back, pm1) == 0) {
        print("modular addition wraps at the modulus");
    }

    // The two masks everything constant-time above is built out of.
    bignum.select(out, x, ones, bignum.zero_mask(bignum.new(8)));
    bignum.select(back, x, ones, bignum.zero_mask(one));
    if (bignum.cmp(out, x) == 0 and bignum.cmp(back, ones) == 0) { print("masks select"); }
    return 0;
}

fn show(a: []u32, n: i64) void {
    const out = bytes.new(n);
    bignum.to_be(a, out);
    print(bytes.to_hex(out));
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
