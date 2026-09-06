// Poly1305 and ChaCha20-Poly1305, against RFC 8439.
//
// Poly1305's accumulator is 130 bits wide, which is the one place item 9's
// decision not to have a `u128` is actually felt. Five 26-bit limbs in `u64`s
// is the answer, and it is the shape every portable implementation uses -- so
// what this case really records is that the decision cost nothing here.
// expect: a8061dc1305136c6c22b8baf0c0127a9
// expect: d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b146a9201d80f083d40ddeaba31d775358591ab3127399ac59c4ab8ac3c461d10ebfb27b1e8d673840bc0dfde5537dabd39e3d5b59704789ef43ad420c4c2920cdf1bd780780317d1fd7e2e9c550f059e
// expect: Ladies and Gentlemen of the class of '99: If I can offer you only one tip for the future, sunscreen would be it.
// expect: tampered ciphertext refused
// expect: tampered tag refused
// expect: tampered aad refused
// expect: wrong nonce refused
// expect: truncated refused
// expect: empty round trip
const cipher = @import("std/cipher");
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    // Section 2.5.2: the one-time key and the message it authenticates.
    print(bytes.to_hex(cipher.poly1305(
        hex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b"),
        bytes.of("Cryptographic Forum Research Group"))));

    // Section 2.8.2, the AEAD end to end.
    const key = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    const nonce = hex("070000004041424344454647");
    const aad = hex("50515253c0c1c2c3c4c5c6c7");
    const plain = bytes.of("Ladies and Gentlemen of the class of '99: If I can offer you only one tip for the future, sunscreen would be it.");
    const sealed = cipher.chacha20_poly1305_seal(key, nonce, aad, plain);
    print(bytes.to_hex(sealed));
    print(bytes.to_str(cipher.chacha20_poly1305_open(key, nonce, aad, sealed) catch return 1));

    // Every way of being wrong has to be refused, and each is a different
    // path: a changed ciphertext, a changed tag, changed additional data that
    // is not itself transmitted, the wrong nonce, and a truncation that leaves
    // no room for a tag at all.
    refuse(key, nonce, aad, flip(sealed, 3), "tampered ciphertext refused");
    refuse(key, nonce, aad, flip(sealed, array.len(sealed) - 1), "tampered tag refused");
    refuse(key, nonce, flip(aad, 0), sealed, "tampered aad refused");
    refuse(key, flip(nonce, 11), aad, sealed, "wrong nonce refused");
    refuse(key, nonce, aad, bytes.slice(sealed, 0, 8), "truncated refused");

    // Nothing to encrypt and nothing to authenticate is still a tag.
    const nothing = bytes.new(0);
    const bare = cipher.chacha20_poly1305_seal(key, nonce, nothing, nothing);
    const back = cipher.chacha20_poly1305_open(key, nonce, nothing, bare) catch return 2;
    if (array.len(bare) == 16 and array.len(back) == 0) { print("empty round trip"); }
    return 0;
}

fn refuse(key: []u8, nonce: []u8, aad: []u8, sealed: []u8, note: str) void {
    const out = cipher.chacha20_poly1305_open(key, nonce, aad, sealed) catch {
        print(note);
        bytes.new(0)
    };
    return;
}

/// A copy of `b` with one bit of byte `at` flipped.
fn flip(b: []u8, at: i64) []u8 {
    const out = bytes.slice(b, 0, array.len(b));
    out[at] ^= 1;
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
