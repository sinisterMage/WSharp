// A literal takes the type it is used at, so `0xff` is a `u8` here and a `u32`
// there with no suffix on either -- and stays an `i64` when nothing asks.
// expect: 255
// expect: 255
// expect: 42
// expect: -128
// expect: 18446744073709551615
// expect: 4294967295
// expect: 16
fn take8(x: u8) u8 { return x; }
fn main() i64 {
    const mask: u8 = 0xff;
    const word: u32 = 0xff;
    print_int(i64(mask));
    print_int(i64(word));

    // Nothing pins this one, so it is an `i64`, as every literal used to be.
    const n = 42;
    print_int(n);

    // The one value an `i8` has that its positive twin does not. A minus sign
    // on a literal is part of the literal, so this is not `-(128)`.
    const low: i8 = -128;
    print_int(i64(low));

    // A `u64` above `i64::MAX`, which the lexer used to reject outright.
    const all: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    print_uint(all);
    print_int(i64(all >> 32));

    // A literal argument takes the parameter's type.
    print_int(i64(take8(0b10000)));
    return 0;
}
