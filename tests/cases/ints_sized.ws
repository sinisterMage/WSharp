// Every integer width, and the wrapping that is the whole point of having
// them: SHA-256 *is* addition modulo 2^32, so a checked `+` would make it
// unwritable. Signed arithmetic wraps here too -- only division panics, which
// is the behaviour `panic_div_overflow.ws` has always pinned.
// expect: 127
// expect: -128
// expect: 255
// expect: 0
// expect: 65535
// expect: 4294967295
// expect: 0
// expect: 2
// expect: -1
fn main() i64 {
    const a: i8 = 127;
    print_int(i64(a));
    print_int(i64(a + 1));

    const b: u8 = 255;
    print_int(i64(b));
    print_int(i64(b + 1));

    const c: u16 = 65535;
    print_int(i64(c));

    const d: u32 = 4294967295;
    print_int(i64(d));
    print_int(i64(d + 1));
    print_int(i64(d * 2 + 4));

    const e: i16 = -1;
    print_int(i64(e));
    return 0;
}
