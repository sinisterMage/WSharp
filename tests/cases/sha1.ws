// SHA-1, which is here because git names every object by one.
//
// It is broken for signatures -- a chosen-prefix collision has been public
// since 2017 -- and nothing in `std/tls` or `std/x509` uses it. A git object id
// is not a signature: it names content the transport above it also checks, and
// a package's own integrity is `ingot/store`'s SHA-256 tree hash.
//
// FIPS 180-4's own vectors, each confirmed against `python3 -c hashlib` before
// being written down -- the same habit that caught a remembered RFC ciphertext
// being wrong while the code was right.
// expect: a9993e364706816aba3e25717850c26c9cd0d89d
// expect: da39a3ee5e6b4b0d3255bfef95601890afd80709
// expect: 84983e441c3bd26ebaae4aa1f95129e5e54670f1
// expect: 34aa973cd4c4daa4f61eeb2bdbad27316534016f
// expect: a9993e364706816aba3e25717850c26c9cd0d89d
const bytes = @import("std/bytes");
const hash = @import("std/hash");

fn digest(s: str) str { return bytes.to_hex(hash.sha1(bytes.of(s))); }

fn main() i64 {
    print(digest("abc"));
    print(digest(""));
    print(digest("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"));

    // A million `a`s, which is the vector that exercises the length counter
    // and the padding across more blocks than anything else here.
    const s = hash.sha1_init();
    const chunk = bytes.new(1000);
    bytes.fill(chunk, 0, 1000, 97);
    var i = 0;
    while (i < 1000) : (i += 1) { hash.sha1_update(s, chunk, 0, 1000); }
    print(bytes.to_hex(hash.sha1_final(s)));

    // Fed one byte at a time, which is the other way the block buffer can be
    // wrong: the incremental answer must be the one-shot answer.
    const t = hash.sha1_init();
    const abc = bytes.of("abc");
    var j = 0;
    while (j < 3) : (j += 1) { hash.sha1_update(t, abc, j, 1); }
    print(bytes.to_hex(hash.sha1_final(t)));
    return 0;
}
