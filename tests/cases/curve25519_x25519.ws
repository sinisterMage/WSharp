// X25519, against RFC 7748's own vectors, checked against a second
// implementation before they were written down here.
//
// The published set is well chosen and is used whole. Section 5.2's two
// vectors pin the ladder; the second also has bit 255 set in its `u`, which
// section 5 requires a receiver to ignore rather than reject, so it pins the
// masking at the same time. Section 6.1 is a whole key exchange: two private
// keys, the two public keys they derive, and the one secret both sides reach.
//
// The iterated test is the strongest single check here and is kept at its
// published length. A thousand rounds is a thousand different scalars against
// a thousand different points, so a reduction that is wrong for one input in a
// million is found by it and by nothing else in this file. It costs about two
// seconds, and it costs the *same* two seconds under `--gc-stress` -- which is
// the other thing it demonstrates, because a ladder that allocated per step
// would be a quarter of a million collections and would never finish.
// expect: c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552
// expect: 95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957
// expect: 8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a
// expect: de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f
// expect: 4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742
// expect: both directions agree
// expect: 422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079
// expect: 684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51
// expect: field identities hold
// expect: nothing kept
const curve = @import("std/curve25519");
const crypto = @import("std/crypto");
const bytes = @import("std/bytes");

fn main() i64 {
    // Section 5.2, both vectors. The second one's `u` ends in 0x93, so its
    // top bit is set and has to be dropped rather than objected to.
    print(bytes.to_hex(curve.x25519(
        hex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"),
        hex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c")) catch return 1));
    print(bytes.to_hex(curve.x25519(
        hex("4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d"),
        hex("e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493")) catch return 2));

    // Section 6.1: Alice and Bob, their public keys, and their shared secret.
    const alice = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
    const bob = hex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
    const alice_pub = curve.x25519_base(alice);
    const bob_pub = curve.x25519_base(bob);
    print(bytes.to_hex(alice_pub));
    print(bytes.to_hex(bob_pub));
    const secret = curve.x25519(alice, bob_pub) catch return 3;
    print(bytes.to_hex(secret));
    // The property the whole function exists for, which no single vector
    // states: the two sides compute the same thing from different halves.
    if (bytes.equal(secret, curve.x25519(bob, alice_pub) catch return 4)) {
        print("both directions agree");
    }

    // Section 5.2's iterated test, after one round and after a thousand.
    var k = hex("0900000000000000000000000000000000000000000000000000000000000000");
    var u = hex("0900000000000000000000000000000000000000000000000000000000000000");
    var i = 0;
    while (i < 1000) : (i += 1) {
        const next = curve.x25519(k, u) catch return 5;
        u = k;
        k = next;
        if (i == 0) { print(bytes.to_hex(k)); }
    }
    print(bytes.to_hex(k));

    // The curve's vectors say nothing about the field beneath it, so the field
    // states its own laws instead: a number times its inverse is one, and a
    // number times one is itself. Over bytes from the system's generator, so
    // this is a different value every run rather than a second fixed vector.
    const r = crypto.random(32) catch return 6;
    // Cleared to well below the prime, so the packed form of `r` is `r`.
    r[31] &= 0x3f;
    const one = bytes.new(32);
    one[0] = 1;
    if (bytes.equal(curve.field_mul(r, curve.field_inv(r)), one)
        and bytes.equal(curve.field_mul(r, one), r)) {
        print("field identities hold");
    }

    // Everything a scalar multiplication allocates, it allocates for itself.
    const before = gc_live_objects();
    i = 0;
    while (i < 8) : (i += 1) {
        const scratch = curve.x25519_base(alice);
        if (scratch[0] == 0xff and scratch[31] == 0xff) { print("unreachable"); }
    }
    gc_collect();
    gc_trace();
    if (gc_live_objects() <= before + 4) { print("nothing kept"); }
    return 0;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
