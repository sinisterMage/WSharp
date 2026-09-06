// `[]u8`, and the bridge between it and `str`.
//
// The bridge is what item 10 needs before anything else: a socket speaks
// `str`, every hash and cipher works in a mutable buffer, and going between
// them a byte at a time through `str.concat` is quadratic. Everything below is
// linear.
// expect: hello
// expect: 5
// expect: 104
// expect: 00ff10
// expect: 3
// expect: 255
// expect: bad
// expect: odd
// expect: deadbeef
// expect: beef
// expect: 12345678
// expect: 78563412
// expect: 0123456789abcdef
// expect: efcdab8967452301
// expect: same
// expect: differs
// expect: length
// expect: f0f0
// expect: aabbccdd
const bytes = @import("std/bytes");
const array = @import("std/array");
const text = @import("std/str");

fn main() i64 {
    // str -> []u8 -> str, unchanged.
    const b = bytes.of("hello");
    print(bytes.to_str(b));
    print_int(array.len(b));
    print_int(i64(b[0]));

    // A buffer written by hand, and hex as the way to read one.
    const raw = bytes.new(3);
    raw[0] = 0;
    raw[1] = 255;
    raw[2] = 16;
    print(bytes.to_hex(raw));

    // Hex back to bytes, and the two ways it can be refused.
    const parsed = bytes.from_hex("00ff10") catch return 1;
    print_int(array.len(parsed));
    print_int(i64(parsed[1]));
    const nope = bytes.from_hex("00zz") catch { print("bad"); bytes.new(0) };
    const odd = bytes.from_hex("abc") catch { print("odd"); bytes.new(0) };

    // Slicing, and slicing to a `str`.
    const d = bytes.from_hex("deadbeef") catch return 2;
    print(bytes.to_hex(d));
    print(bytes.to_hex(bytes.slice(d, 2, 4)));

    // Words, both ends. The same four bytes read two ways.
    const w = bytes.from_hex("12345678") catch return 3;
    print(bytes.to_hex(be(bytes.be32(w, 0))));
    print(bytes.to_hex(be(bytes.le32(w, 0))));
    const g = bytes.from_hex("0123456789abcdef") catch return 4;
    print(bytes.to_hex(be64(bytes.be64(g, 0))));
    print(bytes.to_hex(be64(bytes.le64(g, 0))));

    // Comparison: equal, differing, and differing in length.
    const one = bytes.from_hex("00112233") catch return 5;
    const two = bytes.from_hex("00112233") catch return 6;
    const three = bytes.from_hex("00112234") catch return 7;
    if (bytes.equal(one, two)) { print("same"); }
    if (!bytes.equal(one, three)) { print("differs"); }
    if (!bytes.equal(one, bytes.slice(one, 0, 3))) { print("length"); }

    // Fill and xor in place, which is what every mode of operation is.
    const pad = bytes.new(2);
    bytes.fill(pad, 0, 2, 0xff);
    const mask = bytes.from_hex("0f0f") catch return 8;
    bytes.xor(pad, 0, mask, 0, 2);
    print(bytes.to_hex(pad));

    // Copy, including the overlapping case a shifting buffer makes.
    const room = bytes.new(4);
    bytes.copy(room, 0, bytes.from_hex("aabb") catch return 9, 0, 2);
    bytes.copy(room, 2, room, 0, 2);
    room[2] = 0xcc;
    room[3] = 0xdd;
    print(bytes.to_hex(room));
    return 0;
}

/// A `u32` back into four big-endian bytes, so a word can be printed as hex.
fn be(v: u32) []u8 {
    const out = bytes.new(4);
    bytes.put_be32(out, 0, v);
    return out;
}

fn be64(v: u64) []u8 {
    const out = bytes.new(8);
    bytes.put_be64(out, 0, v);
    return out;
}
