// A `u64` above 2^63 has its top bit set, which is a sign bit to an `i64` and
// a value to a `u64`. Every ordering comparison and both divisions have to
// know which they are looking at.
// expect: bigger
// expect: 9223372036854775808
// expect: 2
// expect: 0
// expect: -1
fn main() i64 {
    const big: u64 = 0x8000000000000000;
    const small: u64 = 1;
    // As a signed value `big` is negative, so a signed compare would say no.
    if (big > small) { print("bigger"); }
    print_uint(big);

    // Likewise the division: signed would give a different answer entirely.
    print_uint(big / 4611686018427387904);
    print_uint(big % 2);

    // And the same bits read as an `i64`, to show the conversion is a
    // reinterpretation rather than a clamp.
    print_int(i64(0xFFFF_FFFF_FFFF_FFFF));
    return 0;
}
