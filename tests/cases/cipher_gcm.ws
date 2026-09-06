// AES-GCM, against the test cases from McGrew and Viega's original proposal --
// the six that NIST's own validation suite is built out of, checked here
// against a second implementation as well as against the published values.
//
// GHASH is written as 128 shifts and exclusive-ors rather than as the
// four-bit table a fast implementation uses, for the reason AES's S-box is
// computed: the table would be indexed by the data being authenticated, which
// under decryption is whatever an attacker sent.
// expect: 58e2fccefa7e3061367f1d57a4e7455a
// expect: 0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf
// expect: 42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f59854d5c2af327cd64a62cf35abd2ba6fab4
// expect: 42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e0915bc94fbc3221a5db94fae95ae7121a47
// expect: 61353b4c2806934a777ff51fa22a4755699b2a714fcdc6f83766e5f97b6c742373806900e49f24b22b097544d4896b424989b5e1ebac0f07c23f45983612d2e79e3b0785561be14aaca2fccb
// expect: 522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f66276fc6ece0f4e1768cddf8853bb2d551b
// expect: round trip
// expect: tampered ciphertext refused
// expect: tampered tag refused
// expect: tampered aad refused
// expect: wrong key refused
const cipher = @import("std/cipher");
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    // Cases 1 and 2: a zero key and a zero nonce, with nothing and then one
    // block of zeros -- which is the pair that pins the hash subkey and the
    // very first counter block.
    const zero = cipher.aes_gcm_init(hex("00000000000000000000000000000000"));
    const zero_iv = hex("000000000000000000000000");
    const nothing = bytes.new(0);
    print(bytes.to_hex(cipher.aes_gcm_seal(zero, zero_iv, nothing, nothing)));
    print(bytes.to_hex(cipher.aes_gcm_seal(zero, zero_iv, nothing, bytes.new(16))));

    const key = hex("feffe9928665731c6d6a8f9467308308");
    const g = cipher.aes_gcm_init(key);
    const iv = hex("cafebabefacedbaddecaf888");
    const full = hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255");
    // Case 4's plaintext is case 3's less the last four bytes, so the final
    // block is short -- which is where a counter-mode implementation goes
    // wrong if it is going to.
    const partial = bytes.slice(full, 0, 60);
    const aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");

    print(bytes.to_hex(cipher.aes_gcm_seal(g, iv, nothing, full)));
    print(bytes.to_hex(cipher.aes_gcm_seal(g, iv, aad, partial)));
    // Case 5: an eight-byte nonce, which is not the twelve-byte special case
    // and so has to be hashed down into a counter block.
    print(bytes.to_hex(cipher.aes_gcm_seal(g, hex("cafebabefacedbad"), aad, partial)));
    // Case 16: the same message under a 256-bit key.
    const g256 = cipher.aes_gcm_init(hex("feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308"));
    print(bytes.to_hex(cipher.aes_gcm_seal(g256, iv, aad, partial)));

    const sealed = cipher.aes_gcm_seal(g, iv, aad, partial);
    if (bytes.equal(cipher.aes_gcm_open(g, iv, aad, sealed) catch return 1, partial)) {
        print("round trip");
    }

    refuse(g, iv, aad, flip(sealed, 0), "tampered ciphertext refused");
    refuse(g, iv, aad, flip(sealed, array.len(sealed) - 1), "tampered tag refused");
    refuse(g, iv, flip(aad, 5), sealed, "tampered aad refused");
    refuse(cipher.aes_gcm_init(flip(key, 0)), iv, aad, sealed, "wrong key refused");
    return 0;
}

fn refuse(g: cipher.AesGcm, iv: []u8, aad: []u8, sealed: []u8, note: str) void {
    const out = cipher.aes_gcm_open(g, iv, aad, sealed) catch {
        print(note);
        bytes.new(0)
    };
    return;
}

fn flip(b: []u8, at: i64) []u8 {
    const out = bytes.slice(b, 0, array.len(b));
    out[at] ^= 1;
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
