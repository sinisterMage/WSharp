// Conversions are written, never inferred: a silent widening is how a 32-bit
// hash becomes a 64-bit one that is right for a while.
// expect: 44
// expect: 4294967295
// expect: -1
// expect: 200
// expect: -56
// expect: 3
// expect: 2
// expect: 9223372036854775807
// expect: 0
fn main() i64 {
    // Truncating says so: 300 does not fit a byte, and 300 - 256 is what is
    // left of it.
    const wide: i64 = 300;
    print_int(i64(u8(wide)));

    // Widening takes its cue from the *source*'s signedness.
    const u: u32 = 0xffffffff;
    print_int(i64(u));
    const s: i32 = -1;
    print_int(i64(s));

    const big: u8 = 200;
    print_int(i64(big));
    print_int(i64(i8(big)));

    // Between the two families.
    print_int(i64(f64(3)));
    print_int(i64(2.9));

    // Float to integer saturates rather than trapping, so the conversion is
    // total: a value too large clamps, and a NaN is zero.
    const huge = 1000000.0 * 1000000.0 * 1000000.0 * 1000000.0;
    print_int(i64(huge));
    print_int(i64(0.0 / 0.0));
    return 0;
}
