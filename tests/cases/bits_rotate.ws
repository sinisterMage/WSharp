// Rotation, which every hash and stream cipher is written in terms of. A
// builtin rather than a shift-shift-or the code generator would have to
// recognise, and generic over the width through `Integer`.
// expect: 878082066
// expect: 2014458966
// expect: 9223372036854775808
// expect: 305419896
// expect: 65
const bits = @import("std/bits");

fn main() i64 {
    const a: u32 = 0x12345678;
    print_int(i64(bits.rotl(a, 8)));
    print_int(i64(bits.rotr(a, 8)));

    const one: u64 = 1;
    print_uint(bits.rotl(one, 63));

    // Rotating by the width is the identity: the amount is masked, exactly as
    // a shift's is.
    print_int(i64(bits.rotl(a, 32)));

    // And on a byte, to show the width comes from the argument.
    const b: u8 = 0b1000_0010;
    print_int(i64(bits.rotr(b, 1)));
    return 0;
}
