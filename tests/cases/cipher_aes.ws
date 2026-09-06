// AES-128 and AES-256, against FIPS 197's own worked examples.
//
// The S-box here is *computed* rather than tabulated -- `sbox` inverts in
// GF(2^8) and applies the affine transform -- because a 256-byte table indexed
// by a byte of the state is indexed by a byte that depends on the key, and
// which cache line that touches is exactly what a timing attack reads. It is
// about a hundred times slower than a lookup. These vectors are what say it is
// nonetheless the same function.
// expect: 3925841d02dc09fbdc118597196a0b32
// expect: 69c4e0d86a7b0430d8cdb78070b4c55a
// expect: 8ea2b7ca516745bfeafc49904b496089
// expect: 99
// expect: 124
// expect: 0
// expect: sbox is a permutation
const cipher = @import("std/cipher");
const bytes = @import("std/bytes");

fn main() i64 {
    // Appendix B, the round-by-round example.
    print(bytes.to_hex(cipher.aes_encrypt(
        cipher.aes_init(hex("2b7e151628aed2a6abf7158809cf4f3c")),
        hex("3243f6a8885a308d313198a2e0370734"))));

    // Appendix C.1 and C.3: the same input under a 128-bit and a 256-bit key.
    const input = hex("00112233445566778899aabbccddeeff");
    print(bytes.to_hex(cipher.aes_encrypt(
        cipher.aes_init(hex("000102030405060708090a0b0c0d0e0f")), input)));
    print(bytes.to_hex(cipher.aes_encrypt(
        cipher.aes_init(hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")),
        input)));

    // Three published entries of the S-box: 0x63, 0x7c, and the zero that
    // 0x52 maps to -- printed as decimal, because `print_int` is.
    const box = sbox_table();
    print_int(i64(box[0]));
    print_int(i64(box[1]));
    print_int(i64(box[82]));

    // And that it is a permutation, which is the property the whole cipher
    // rests on and the one a wrong inversion would break.
    const seen = bytes.new(256);
    var i = 0;
    while (i < 256) : (i += 1) { seen[i64(box[i])] += 1; }
    var all_once = true;
    i = 0;
    while (i < 256) : (i += 1) {
        if (seen[i] != 1) { all_once = false; }
    }
    if (all_once) { print("sbox is a permutation"); }
    return 0;
}

fn sbox_table() []u8 {
    const out = bytes.new(256);
    var i = 0;
    while (i < 256) : (i += 1) { out[i] = cipher.aes_sub_byte(u8(i)); }
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
