// The operators item 9 exists for, and the precedence they were given: looser
// than `+`, tighter than comparison. That last one is C's famous bug fixed --
// `flags & MASK == MASK` means what it reads as.
// expect: 48
// expect: 255
// expect: 240
// expect: -1
// expect: 1024
// expect: -4
// expect: 8
// expect: masked
// expect: 18
fn main() i64 {
    print_int(0xf0 & 0x3c);
    print_int(0xf0 | 0x0f);
    print_int(0xff ^ 0x0f);
    print_int(~0);
    print_int(1 << 10);
    print_int(-16 >> 2);

    // Shifts bind tighter than `&` and looser than `+`, so this is `1 << 3`.
    print_int(1 << 2 + 1);

    if (0xf0 & 0x10 == 0x10) { print("masked"); }

    // Every compound form, in order: 1 -> 16 -> 19 -> 19 -> 18.
    var x = 1;
    x <<= 4;
    x |= 3;
    x &= 0x1f;
    x ^= 1;
    print_int(x);
    return 0;
}
