// HKDF-SHA-256, against RFC 5869's three SHA-256 test cases.
//
// All three, because the two beyond the first are where the corners are: case
// 2 asks for 82 bytes, which is more than one HMAC block of output and so
// exercises the counter and the chaining; case 3 supplies neither a salt nor
// an info string, which is the shape TLS 1.3's key schedule uses more than any
// other and the one where an implementation that quietly substitutes something
// for an empty salt gives a different answer.
// expect: 077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5
// expect: 3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865
// expect: 06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244
// expect: b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87
// expect: 19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04
// expect: 8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8
// expect: short output
const hash = @import("std/hash");
const bytes = @import("std/bytes");

fn main() i64 {
    const sha256 = hash.sha256_hash();

    // Case 1: 22 bytes of input keying material, a 13-byte salt, 42 out.
    const prk1 = hash.hkdf_extract(sha256, hex("000102030405060708090a0b0c"),
                                   filled(22, 0x0b));
    print(bytes.to_hex(prk1));
    print(bytes.to_hex(hash.hkdf_expand(sha256, prk1, hex("f0f1f2f3f4f5f6f7f8f9"), 42)));

    // Case 2: eighty bytes of everything, and 82 out -- three HMAC blocks,
    // with the last one cut short.
    const prk2 = hash.hkdf_extract(sha256, counting(0x60, 80), counting(0x00, 80));
    print(bytes.to_hex(prk2));
    print(bytes.to_hex(hash.hkdf_expand(sha256, prk2, counting(0xb0, 80), 82)));

    // Case 3: no salt and no info at all.
    const nothing = bytes.new(0);
    const prk3 = hash.hkdf_extract(sha256, nothing, filled(22, 0x0b));
    print(bytes.to_hex(prk3));
    print(bytes.to_hex(hash.hkdf_expand(sha256, prk3, nothing, 42)));

    // Less than one block out is the case a loop written the obvious way gets
    // wrong, so it is asserted rather than assumed: the first 16 bytes of the
    // 42 above.
    const full = hash.hkdf_expand(sha256, prk3, nothing, 42);
    const part = hash.hkdf_expand(sha256, prk3, nothing, 16);
    if (bytes.equal(part, bytes.slice(full, 0, 16))) { print("short output"); }
    return 0;
}

fn hex(s: str) []u8 {
    return bytes.from_hex(s) catch bytes.new(0);
}

fn filled(n: i64, v: u8) []u8 {
    const out = bytes.new(n);
    bytes.fill(out, 0, n, v);
    return out;
}

/// `n` bytes counting up from `first`, which is how RFC 5869 writes case 2's
/// three eighty-byte inputs.
fn counting(first: i64, n: i64) []u8 {
    const out = bytes.new(n);
    var i = 0;
    while (i < n) : (i += 1) { out[i] = u8(first + i); }
    return out;
}
