// A top-level `const` array: the shape every hash and cipher is written
// around, and the thing item 10 asked the language for first.
//
// It lives in the data section beside the string literals, immortal and
// holding no references -- so naming it costs an address, mentioning it inside
// a loop allocates nothing, and the collector never has to look at it. That
// last part is the whole argument: a computed global would need a startup
// initialiser and would need the collector to treat it as a root, and an array
// of scalars needs neither.
// expect: 1116352408
// expect: 1899447441
// expect: 148
// expect: 255
// expect: -128
// expect: 2
// expect: 1
// expect: no allocation
const array = @import("std/array");

// The first four of SHA-256's round constants, which is what this is for.
const K = []u32{ 0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5 };
// Every element type a table can have, to pin the stride at each width.
const BYTES = []u8{ 0, 255, 16 };
const SMALL = []i8{ -128, 127 };
const WIDE = []i64{ 1, 2, 3 };
const FLAGS = []bool{ false, true };
const RATIOS = []f64{ 0.5, 2.5 };

fn total(k: []u32) u64 {
    var sum: u64 = 0;
    for (k) |w| { sum += u64(w); }
    return sum;
}

fn main() i64 {
    print_int(i64(K[0]));
    print_int(i64(K[1]));
    // Read in a loop, through a function, to show it is an ordinary `[]u32`
    // once it has been named.
    print_int(i64(total(K) >> 26));
    print_int(i64(BYTES[1]));
    print_int(i64(SMALL[0]));
    print_int(i64(WIDE[1]));
    if (FLAGS[1]) { print_int(i64(RATIOS[1] * 0.4)); }

    // The point of the whole thing: naming it in a loop allocates nothing.
    // `array.len` reads the header, so the count is the one the data carries.
    const before = gc_live_objects();
    var i = 0;
    var seen = 0;
    while (i < 1000) : (i += 1) { seen += i64(K[i % array.len(K)]); }
    if (gc_live_objects() == before and seen > 0) { print("no allocation"); }
    return 0;
}
