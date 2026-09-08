// An `f64`'s representation, both ways.
//
// `u64(x)` converts a *value* and rounds to say it: `u64(1.5)` is 1. These
// answer what the value is *made of*, which is a different question and the
// only one an IEEE-754 codec can use -- a wire format carries the eight bytes,
// not the number. Without them a decoder has to rebuild the double out of
// sign, exponent and fraction by arithmetic, which is exact but is ninety lines
// where this is one call.
//
// The bit patterns are IEEE-754 binary64 and were taken from Python's
// `struct.pack('<d', v)`, which is a second implementation rather than a
// memory: both ends of every line below were checked against it.
// expect: 4607182418800017408
// expect: 13830554455654793216
// expect: 0
// expect: 9223372036854775808
// expect: 4611686018427387904
// expect: 4602678819172646912
// expect: 4614256650576692846
// expect: 1
// expect: 9218868437227405311
// expect: 4503599627370496
// expect: 9218868437227405312
// expect: 18442240474082181120
// expect: 1.0
// expect: 0.0
// expect: 5e-324 round trips: true
// expect: max finite round trips: true
// expect: negative zero survives: true
// expect: it is not positive zero: true
const bits = @import("std/bits");

fn main() i64 {
    print_uint(bits.f64_bits(1.0));
    print_uint(bits.f64_bits(-1.0));
    print_uint(bits.f64_bits(0.0));
    print_uint(bits.f64_bits(-0.0));
    print_uint(bits.f64_bits(2.0));
    print_uint(bits.f64_bits(0.5));
    print_uint(bits.f64_bits(3.14159));

    // The range ends: the smallest subnormal, the largest finite value, the
    // smallest normal, and both infinities. W# has no literal for the last
    // three, which is exactly why decoding them has to work.
    print_uint(bits.f64_bits(bits.f64_from_bits(u64(1))));
    print_uint(bits.f64_bits(bits.f64_from_bits(u64(9218868437227405311))));
    print_uint(bits.f64_bits(bits.f64_from_bits(u64(4503599627370496))));
    print_uint(bits.f64_bits(1.0 / 0.0));
    print_uint(bits.f64_bits(-1.0 / 0.0));

    print_float(bits.f64_from_bits(u64(4607182418800017408)));
    print_float(bits.f64_from_bits(u64(0)));

    // A round trip through the bits is the identity, including where the value
    // has no literal to write it with.
    const tiny = bits.f64_from_bits(u64(1));
    print(str_of(bits.f64_bits(tiny) == u64(1), "5e-324 round trips"));
    const big = bits.f64_from_bits(u64(9218868437227405311));
    print(str_of(
        bits.f64_bits(big) == u64(9218868437227405311),
        "max finite round trips",
    ));

    // Negative zero is the value `==` cannot tell apart from zero, and the one
    // a codec most easily loses.
    const neg = -0.0;
    print(str_of(bits.f64_bits(neg) == u64(9223372036854775808), "negative zero survives"));
    print(str_of(bits.f64_bits(neg) != bits.f64_bits(0.0), "it is not positive zero"));
    return 0;
}

const str = @import("std/str");

fn str_of(ok: bool, what: str) str {
    if (ok) { return str.concat(what, ": true"); }
    return str.concat(what, ": false");
}
