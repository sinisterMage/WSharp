// The growable buffer, the 16- and 24-bit accessors, and base64.
//
// Three additions `std/bytes` needed for item 10's protocol half, and one case
// for all three because each exists only to serve the others: every message in
// TLS and every value in DER is written `length, then contents`, and the
// length is not known until the contents are -- so a buffer that can go back
// and fill one in is what the encoders are written with. `open16` writes a
// placeholder and answers with where it went; `close16` returns to it.
// `open32`/`close32` are the fourth width, which a PostgreSQL frame header
// needs and TLS does not.
//
// The base64 half is for PEM, which is the format a Unix machine keeps its
// certificate store in. It is checked against Python's `base64`, including
// both padded lengths, which is where an encoder gets it wrong.
// expect: 00000c16000968656c6c6fdeadbeef
// expect: 15
// expect: 16000968656c6c6fdeadbeef
// expect: 513
// expect: 65535
// expect: abcdef
// expect: 11259375
// expect: the buffer grew and kept its contents
// expect: aGVsbG8sIHdvcmxk
// expect: aGVsbA==
// expect: aGU=
// expect: hello, world
// expect: hell
// expect: he
// expect: base64 with newlines in it decodes
// expect: a character outside the alphabet refused
// expect: a digit after the padding refused
// expect: three padding characters refused
// expect: 0000000916000668656c6c6f21
// expect: 9
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    // A three-byte length around a two-byte length around some contents, which
    // is the shape of a handshake message holding an extension.
    const b = bytes.buf(4);
    const outer = bytes.open24(b);
    bytes.put_u8(b, 0x16);
    const inner = bytes.open16(b);
    bytes.put_str(b, "hello");
    bytes.put_u32(b, 0xdeadbeef);
    bytes.close16(b, inner);
    bytes.close24(b, outer);
    print(bytes.to_hex(bytes.taken(b)));
    print_int(b.used);

    // What a record reader does with the bytes it has consumed.
    bytes.drop_front(b, 3);
    print(bytes.to_hex(bytes.taken(b)));

    const w = bytes.new(4);
    bytes.put_be16(w, 0, 513);
    bytes.put_be16(w, 2, 65535);
    print_int(bytes.be16(w, 0));
    print_int(bytes.be16(w, 2));
    const t = bytes.new(3);
    bytes.put_be24(t, 0, 0xabcdef);
    print(bytes.to_hex(t));
    print_int(bytes.be24(t, 0));

    // Past the initial capacity several times over: the doubling has to keep
    // what was already written, which is the one thing a grow can get wrong.
    const g = bytes.buf(1);
    var i = 0;
    while (i < 5000) : (i += 1) { bytes.put_u8(g, i & 0xff); }
    const grown = bytes.taken(g);
    if (array.len(grown) == 5000 and grown[0] == 0 and grown[4999] == u8(4999 & 0xff)
        and grown[256] == 0) {
        print("the buffer grew and kept its contents");
    }

    // Both padded lengths and the unpadded one.
    print(bytes.to_base64(bytes.of("hello, world")));
    print(bytes.to_base64(bytes.of("hell")));
    print(bytes.to_base64(bytes.of("he")));
    print(bytes.to_str(bytes.from_base64("aGVsbG8sIHdvcmxk") catch return 1));
    print(bytes.to_str(bytes.from_base64("aGVsbA==") catch return 2));
    print(bytes.to_str(bytes.from_base64("aGU=") catch return 3));

    // PEM wraps at 64 columns, so a decoder that refused a newline could not
    // read the thing it exists to read.
    if (bytes.equal(bytes.from_base64("aGVsbG8s\nIHdvcmxk\n") catch bytes.new(0),
                    bytes.of("hello, world"))) {
        print("base64 with newlines in it decodes");
    }

    refuse("aGU!", "a character outside the alphabet refused");
    refuse("aGU=bG8=", "a digit after the padding refused");
    refuse("aA===", "three padding characters refused");

    // The same shape again at 32 bits: a frame whose header is a four-byte
    // length, holding a message whose own length is two bytes.
    const f = bytes.buf(4);
    const frame = bytes.open32(f);
    bytes.put_u8(f, 0x16);
    const body = bytes.open16(f);
    bytes.put_str(f, "hello!");
    bytes.close16(f, body);
    bytes.close32(f, frame);
    print(bytes.to_hex(bytes.taken(f)));
    print_int(i64(bytes.be32(bytes.taken(f), 0)));
    return 0;
}

fn refuse(s: str, note: str) void {
    bytes.from_base64(s) catch { print(note); return; };
    return;
}
