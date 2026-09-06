// ChaCha20, against RFC 8439's own vectors.
//
// The block function first, because everything else is built on it and
// because section 2.3.2 prints the keystream block on its own -- so a wrong
// answer here is localised rather than smeared across a whole message. Then
// the stream, which is the block function plus a counter and an exclusive-or.
// expect: 10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e
// expect: 6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0bf91b65c5524733ab8f593dabcd62b35718229a2fa05851bfca584339d0122fdb10861bf15a437b2852ea98479e6db01643f4521804ccfe16dfc9b64faa7f3a375cee00fa6dec59e5a54788a8b0740a05
// expect: Ladies and Gentlemen of the class of '99: If I can offer you only one tip for the future, sunscreen would be it.
// expect: counter advances
const cipher = @import("std/cipher");
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    // Section 2.3.2: key 00..1f, nonce 000000090000004a00000000, counter 1.
    const key = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    print(bytes.to_hex(cipher.chacha20_block(key, hex("000000090000004a00000000"), 1)));

    // Section 2.4.2: the same key, a different nonce, and 114 bytes -- which
    // is one block and a bit, so the counter has to advance and the last
    // block has to be used short.
    const nonce = hex("000000000000004a00000000");
    const text = bytes.of("Ladies and Gentlemen of the class of '99: If I can offer you only one tip for the future, sunscreen would be it.");
    cipher.chacha20_xor(key, nonce, 1, text, 0, array.len(text));
    print(bytes.to_hex(text));

    // A stream cipher is its own inverse, so running it again gives the
    // plaintext back. That is also the property the AEAD's `open` relies on.
    cipher.chacha20_xor(key, nonce, 1, text, 0, array.len(text));
    print(bytes.to_str(text));

    // The second block of a stream must equal the block function at counter
    // 2 -- which is the check that the counter advances rather than the
    // keystream repeating, the single worst thing a stream cipher can do.
    const zeros = bytes.new(128);
    cipher.chacha20_xor(key, nonce, 1, zeros, 0, 128);
    if (bytes.equal(bytes.slice(zeros, 64, 128), cipher.chacha20_block(key, nonce, 2))) {
        print("counter advances");
    }
    return 0;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
