// Scalars pack: a `u8` field costs a byte and a `[]u8` has a stride of one.
// Three layouts have to agree about that -- a struct's fields, a subtype's,
// and a closure's captures -- and they agree because all three go through
// `layout::place`.
// expect: 1
// expect: 2
// expect: 70000
// expect: 9
// expect: 4
// expect: 255
// expect: 66
// expect: 1030
const Head = struct { a: u8, b: u8, wide: i32 };
const Tail = struct : Head { c: u8, tag: str };

fn main() i64 {
    // A run of narrow fields, then a wider one that has to be aligned past
    // them rather than laid on top of them.
    const t = Tail{ .a = 1, .b = 2, .wide = 70000, .c = 9, .tag = "tail" };
    print_int(i64(t.a));
    print_int(i64(t.b));
    print_int(i64(t.wide));
    print_int(i64(t.c));

    // A subtype's fields are its supertype's followed by its own, so a read
    // compiled against `Head` runs unchanged on a `Tail`.
    print_int(i64(count_fields(t)));

    // Bytes in an array, at a stride of one.
    const bytes = []u8{ 255, 66, 0, 0 };
    print_int(i64(bytes[0]));
    print_int(i64(bytes[1]));

    // A closure capturing narrow values reads them back out of an environment
    // laid out the same way.
    const w: i32 = 1024;
    const small: u8 = 6;
    const add = fn () i32 { return w + i32(small); };
    print_int(i64(add()));
    return 0;
}

fn count_fields(h: Head) u8 { return h.a + h.b + h.a; }
