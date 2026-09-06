// ECDH and ECDSA on NIST P-384.
//
// Two published sources, because the two operations have different ones:
// RFC 5903 section 8.2 is a whole ECDH exchange -- two private keys, the two
// public points, and the one secret both sides reach from opposite halves --
// and RFC 6979 appendix A.2.6 is six ECDSA signatures, which exist to be
// written down because that specification is about *deterministic* nonces.
// All six were handed to node's `crypto.verify` before they were written here.
//
// **This curve is the whole reason `std/nistec` exists.** Thirty-five of this
// machine's root certificates have P-384 keys, and until it was written not
// one of them could be used; a chain through a P-384 intermediate -- which is
// most of the modern web -- could not be verified at all. What it cost was
// these tables and a limb count, because stage two chose `std/bignum`'s
// generic Montgomery multiplication over a fast reduction written for one
// prime. This case is the receipt for that decision.
//
// The first line is the check that costs nothing and catches the most: one
// times the base point must give back the base point. A wrong modulus, a
// wrong Montgomery form or a ladder with its bits the wrong way up all fail
// there rather than somewhere subtler.
// expect: 04aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab73617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f
// expect: 04667842d7d180ac2cde6f74f37551f55755c7645c20ef73e31634fe72b4c55ee6de3ac808acb4bdb4c88732aee95f41aa9482ed1fc0eeb9cafc4984625ccfc23f65032149e0e144ada024181535a0f38eeb9fcff3c2c947dae69b4c634573a81c
// expect: 04e558dbef53eecde3d3fccfc1aea08a89a987475d12fd950d83cfa41732bc509d0d1ac43a0336def96fda41d0774a3571dcfbec7aacf3196472169e838430367f66eebe3c6e70c416dd5f0c68759dd1fff83fa40142209dff5eaad96db9e6386c
// expect: 11187331c279962d93d604243fd592cb9d0a926f422e47187521287e7156c5c4d603135569b9e9d09cf5d4a270f59746
// expect: both directions agree
// expect: a derived key is a valid one
// expect: sha-256 over "sample" verifies
// expect: sha-384 over "sample" verifies
// expect: sha-512 over "sample" verifies
// expect: sha-256 over "test" verifies
// expect: sha-384 over "test" verifies
// expect: sha-512 over "test" verifies
// expect: a flipped signature byte refused
// expect: a P-256 signature is not a P-384 one
// expect: nothing kept
const nistec = @import("std/nistec");
const hash = @import("std/hash");
const bytes = @import("std/bytes");
const array = @import("std/array");

/// RFC 5903 section 8.2: the initiator's i and the responder's r.
const I = "099f3c7034d4a2c699884d73a375a67f7624ef7c6b3c0f160647b67414dce655e35b538041e649ee3faef896783ab194";
const R = "41cb0779b4bdb85d47846725fbec3c9430fab46cc8dc5060855cc9bda0aa2942e0308312916b8ed2960e4bd55a7448fc";
const GI = "04667842d7d180ac2cde6f74f37551f55755c7645c20ef73e31634fe72b4c55ee6de3ac808acb4bdb4c88732aee95f41aa9482ed1fc0eeb9cafc4984625ccfc23f65032149e0e144ada024181535a0f38eeb9fcff3c2c947dae69b4c634573a81c";
const GR = "04e558dbef53eecde3d3fccfc1aea08a89a987475d12fd950d83cfa41732bc509d0d1ac43a0336def96fda41d0774a3571dcfbec7aacf3196472169e838430367f66eebe3c6e70c416dd5f0c68759dd1fff83fa40142209dff5eaad96db9e6386c";

/// RFC 6979 appendix A.2.6's key, and its signatures.
const X = "6b9d3dad2e1b8c1c05b19875b6659f4de23c3b667bf297ba9aa47740787137d896d5724e4c70a825f872c9ea60d2edf5";
const PUB = "04ec3a4e415b4e19a4568618029f427fa5da9a8bc4ae92e02e06aae5286b300c64def8f0ea9055866064a254515480bc138015d9b72d7d57244ea8ef9ac0c621896708a59367f9dfb9f54ca84b3f1c9db1288b231c3ae0d4fe7344fd2533264720";
const SIG_256_SAMPLE = "3065023021b13d1e013c7fa1392d03c5f99af8b30c570c6f98d4ea8e354b63a21d3daa33bde1e888e63355d92fa2b3c36d8fb2cd023100f3aa443fb107745bf4bd77cb3891674632068a10ca67e3d45db2266fa7d1feebefdc63eccd1ac42ec0cb8668a4fa0ab0";
const SIG_384_SAMPLE = "306602310094edbb92a5ecb8aad4736e56c691916b3f88140666ce9fa73d64c4ea95ad133c81a648152e44acf96e36dd1e80fabe4602310099ef4aeb15f178cea1fe40db2603138f130e740a19624526203b6351d0a3a94fa329c145786e679e7b82c71a38628ac8";
const SIG_512_SAMPLE = "3065023100ed0959d5880ab2d869ae7f6c2915c6d60f96507f9cb3e047c0046861da4a799cfe30f35cc900056d7c99cd78824337090230512c8cceee3890a84058ce1e22dbc2198f42323ce8aca9135329f03c068e5112dc7cc3ef3446defceb01a45c2667fdd5";
const SIG_256_TEST = "306402306d6defac9ab64dabafe36c6bf510352a4cc27001263638e5b16d9bb51d451559f918eedaf2293be5b475cc8f0188636b02302d46f3becbcc523d5f1a1256bf0c9b024d879ba9e838144c8ba6baeb4b53b47d51ab373f9845c0514eefb14024787265";
const SIG_384_TEST = "30660231008203b63d3c853e8d77227fb377bcf7b7b772e97892a80f36ab775d509d7a5feb0542a7f0812998da8f1dd3ca3cf023db023100ddd0760448d42d8a43af45af836fce4de8be06b485e9b61b827c2f13173923e06a739f040649a667bf3b828246baa5a5";
const SIG_512_TEST = "3066023100a0d5d090c9980faf3c2ce57b7ae951d31977dd11c775d314af55f76c676447d06fb6495cd21b4b6e340fc236584fb277023100976984e59b4c77b0e8e4460dca3d9f20e07b9bb1f63beefaf576f6b2e8b224634a2092cd3792e0159ad9cee37659c736";

fn main() i64 {
    const c = nistec.p384();
    const one = bytes.new(48);
    one[47] = 1;
    print(bytes.to_hex(nistec.derive(c, one) catch return 1));

    const gi = nistec.derive(c, hex(I)) catch return 2;
    const gr = nistec.derive(c, hex(R)) catch return 3;
    print(bytes.to_hex(gi));
    print(bytes.to_hex(gr));
    const secret = nistec.ecdh(c, hex(I), gr) catch return 4;
    print(bytes.to_hex(secret));
    if (bytes.equal(secret, nistec.ecdh(c, hex(R), gi) catch return 5)) {
        print("both directions agree");
    }
    if (nistec.valid(c, gi) and nistec.valid(c, gr)) {
        print("a derived key is a valid one");
    }

    // The signatures. The three hashes matter separately: SEC1 takes the
    // leftmost 384 bits of the digest, so a SHA-384 signature uses all of it,
    // a SHA-512 one uses three quarters, and a SHA-256 one is a *shorter*
    // number than the curve -- which is the case a verifier that assumed the
    // digest was at least as wide as the order gets wrong.
    const pk = hex(PUB);
    const sample = bytes.of("sample");
    const test = bytes.of("test");
    check(c, pk, hash.sha256(sample), SIG_256_SAMPLE, "sha-256 over \"sample\" verifies");
    check(c, pk, hash.sha384(sample), SIG_384_SAMPLE, "sha-384 over \"sample\" verifies");
    check(c, pk, hash.sha512(sample), SIG_512_SAMPLE, "sha-512 over \"sample\" verifies");
    check(c, pk, hash.sha256(test), SIG_256_TEST, "sha-256 over \"test\" verifies");
    check(c, pk, hash.sha384(test), SIG_384_TEST, "sha-384 over \"test\" verifies");
    check(c, pk, hash.sha512(test), SIG_512_TEST, "sha-512 over \"test\" verifies");

    const forged = hex(SIG_384_SAMPLE);
    forged[array.len(forged) - 1] ^= 1;
    if (!nistec.ecdsa_verify(c, pk, hash.sha384(sample), forged)) {
        print("a flipped signature byte refused");
    }
    // The same bytes on the wrong curve: a 97-byte point is not a 65-byte one,
    // and the length check is what says so before any arithmetic runs.
    if (!nistec.ecdsa_verify(nistec.p256(), pk, hash.sha384(sample),
                             hex(SIG_384_SAMPLE))) {
        print("a P-256 signature is not a P-384 one");
    }

    // A verification allocates for itself and keeps nothing, which is what
    // lets a chain of them cost one certificate's worth each.
    const digest = hash.sha384(sample);
    const sig = hex(SIG_384_SAMPLE);
    const before = gc_live_objects();
    var n = 0;
    while (n < 3) : (n += 1) {
        if (!nistec.ecdsa_verify(c, pk, digest, sig)) { print("unreachable"); }
    }
    gc_collect();
    gc_trace();
    if (gc_live_objects() <= before + 4) { print("nothing kept"); }
    return 0;
}

fn check(c: nistec.Curve, pk: []u8, digest: []u8, sig: str, note: str) void {
    if (nistec.ecdsa_verify(c, pk, digest, hex(sig))) { print(note); }
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
