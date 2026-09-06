// ChaCha20's quarter round, and the test vector from RFC 8439 section 2.1.1.
//
// This is the program item 9 exists for. Every line of it needs something the
// language did not have: 32-bit words, addition that wraps rather than panics,
// `^`, and a rotate. Written with the compound forms on purpose, since those
// are how the algorithm is actually spelled.
// expect: 3928658676
// expect: 3407673550
// expect: 1166100270
// expect: 1484899515
const bits = @import("std/bits");

const Quarter = struct { a: u32, b: u32, c: u32, d: u32 };

fn round(s: Quarter) Quarter {
    var a = s.a;
    var b = s.b;
    var c = s.c;
    var d = s.d;

    a += b;  d ^= a;  d = bits.rotl(d, 16);
    c += d;  b ^= c;  b = bits.rotl(b, 12);
    a += b;  d ^= a;  d = bits.rotl(d, 8);
    c += d;  b ^= c;  b = bits.rotl(b, 7);

    return Quarter{ .a = a, .b = b, .c = c, .d = d };
}

fn main() i64 {
    const out = round(Quarter{
        .a = 0x11111111,
        .b = 0x01020304,
        .c = 0x9b8d6f43,
        .d = 0x01234567,
    });
    // 0xea2a92f4, 0xcb1cf8ce, 0x4581472e, 0x5881c4bb
    print_int(i64(out.a));
    print_int(i64(out.b));
    print_int(i64(out.c));
    print_int(i64(out.d));
    return 0;
}
