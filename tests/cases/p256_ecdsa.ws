// ECDSA on NIST P-256, against RFC 6979 appendix A.2.5.
//
// RFC 6979 is a specification for *deterministic* nonces, which is why it has
// published signatures at all: an ordinary ECDSA vector cannot be reproduced,
// because the signer picks `k` at random. That makes A.2.5 the one place a
// P-256 signature comes with its exact `(r, s)` written down beside the key
// and the message.
//
// Six of them, three hashes over each of two messages. The three hashes matter
// separately here: SEC1 takes the *leftmost* 256 bits of the digest, so a
// SHA-256 signature uses all of it and a SHA-512 one uses half, and a verifier
// that reduced the whole digest modulo n instead would pass every SHA-256 line
// and fail every SHA-512 one.
//
// Each signature was DER-encoded and handed to node's `crypto.verify` before
// it was written here, and all six agreed. The key's public point is checked
// first, against the RFC's own Ux and Uy, because a wrong base point or a
// wrong Montgomery form fails there rather than somewhere subtler.
// expect: 0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299
// expect: sha-256 over "sample" verifies
// expect: sha-384 over "sample" verifies
// expect: sha-512 over "sample" verifies
// expect: sha-256 over "test" verifies
// expect: sha-384 over "test" verifies
// expect: sha-512 over "test" verifies
// expect: nothing kept
const nistec = @import("std/nistec");
const hash = @import("std/hash");
const bytes = @import("std/bytes");

/// RFC 6979 appendix A.2.5's key pair.
const X = "c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721";
const PUB = "0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299";

const SIG_256_SAMPLE = "3046022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const SIG_384_SAMPLE = "304402200eafea039b20e9b42309fb1d89e213057cbf973dc0cfc8f129edddc800ef771902204861f0491e6998b9455193e34e7b0d284ddd7149a74b95b9261f13abde940954";
const SIG_512_SAMPLE = "30450221008496a60b5e9b47c825488827e0495b0e3fa109ec4568fd3f8d1097678eb97f0002202362ab1adbe2b8adf9cb9edab740ea6049c028114f2460f96554f61fae3302fe";
const SIG_256_TEST = "3045022100f1abb023518351cd71d881567b1ea663ed3efcf6c5132b354f28d3b0b7d383670220019f4113742a2b14bd25926b49c649155f267e60d3814b4c0cc84250e46f0083";
const SIG_384_TEST = "304602210083910e8b48bb0c74244ebdf7f07a1c5413d61472bd941ef3920e623fbccebeb60221008ddbec54cf8cd5874883841d712142a56a8d0f218f5003cb0296b6b509619f2c";
const SIG_512_TEST = "30440220461d93f31b6540894788fd206c07cfa0cc35f46fa3c91816fff1040ad1581a04022039af9f15de0db8d97e72719c74820d304ce5226e32dedae67519e840d1194e55";

fn main() i64 {
    const p256 = nistec.p256();

    // The key the signatures belong to, derived rather than transcribed.
    const pk = nistec.derive(p256, hex(X)) catch return 1;
    print(bytes.to_hex(pk));

    const sample = bytes.of("sample");
    const test = bytes.of("test");
    if (nistec.ecdsa_verify(p256, pk, hash.sha256(sample), hex(SIG_256_SAMPLE))) {
        print("sha-256 over \"sample\" verifies");
    }
    if (nistec.ecdsa_verify(p256, pk, hash.sha384(sample), hex(SIG_384_SAMPLE))) {
        print("sha-384 over \"sample\" verifies");
    }
    if (nistec.ecdsa_verify(p256, pk, hash.sha512(sample), hex(SIG_512_SAMPLE))) {
        print("sha-512 over \"sample\" verifies");
    }
    if (nistec.ecdsa_verify(p256, pk, hash.sha256(test), hex(SIG_256_TEST))) {
        print("sha-256 over \"test\" verifies");
    }
    if (nistec.ecdsa_verify(p256, pk, hash.sha384(test), hex(SIG_384_TEST))) {
        print("sha-384 over \"test\" verifies");
    }
    if (nistec.ecdsa_verify(p256, pk, hash.sha512(test), hex(SIG_512_TEST))) {
        print("sha-512 over \"test\" verifies");
    }

    // A verification allocates for itself and keeps nothing, which is what
    // lets `std/tls` do one per certificate in a chain.
    const digest = hash.sha256(sample);
    const sig = hex(SIG_256_SAMPLE);
    const before = gc_live_objects();
    var i = 0;
    while (i < 3) : (i += 1) {
        if (!nistec.ecdsa_verify(p256, pk, digest, sig)) { print("unreachable"); }
    }
    gc_collect();
    gc_trace();
    if (gc_live_objects() <= before + 4) { print("nothing kept"); }
    return 0;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
