// `>>` is the one operator whose *instruction* depends on signedness, which is
// most of the reason the two kinds of integer are worth distinguishing: an
// arithmetic shift keeps the sign, a logical one shifts zeroes in.
// expect: -128
// expect: 128
// expect: -1
// expect: 255
// expect: 305419896
// expect: 152709948
fn arith(x: i32) i32 { return x >> 24; }
fn logic(x: u32) u32 { return x >> 24; }
fn main() i64 {
    print_int(i64(arith(-2147483648)));
    print_int(i64(logic(0x80000000)));

    print_int(i64(arith(-1)));
    print_int(i64(logic(0xffffffff)));

    // Cranelift masks the shift amount to the operand's width, so a shift by
    // the width is a shift by nothing rather than undefined behaviour. That is
    // the rule W# adopts, because it is what both targets do in hardware.
    const a: u32 = 0x12345678;
    print_int(i64(a << 32));
    // ...and one more than the width is a shift by one: 0x12345678 >> 1.
    print_int(i64(a >> 33));
    return 0;
}
