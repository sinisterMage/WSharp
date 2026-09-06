// Every way an ECDSA signature can fail to be one, a line each.
//
// Half of these are refusals by `std/der` rather than by the curve, and that
// is the half worth having. A signature is a byte string a peer chose, and
// every extra encoding a verifier accepts is a second name for the same
// signature -- which is how a protocol that hashes a signature, or counts
// them, or remembers having seen one, comes apart. So a non-minimal INTEGER,
// an integer with its sign bit set, an indefinite length and one trailing byte
// are all refused, and each gets its own line because a single "a bad
// signature is refused" check passes with any of them accepted.
//
// The other half are the curve's: `r` or `s` at zero makes the verification
// equation degenerate, and either at or above the group order is again a
// second encoding of a scalar.
//
// All thirteen were handed to node's `crypto.verify` first and all thirteen
// agree, acceptance included.
// expect: the published signature is accepted
// expect: a flipped r refused
// expect: a flipped s refused
// expect: a signature of another message refused
// expect: r of zero refused
// expect: s of zero refused
// expect: r at the group order refused
// expect: s at the group order refused
// expect: a non-minimal integer refused
// expect: an integer with its sign bit set refused
// expect: one trailing byte refused
// expect: an indefinite length refused
// expect: a truncated signature refused
// expect: a SET where a SEQUENCE belongs refused
// expect: a public key off the curve refused
const nistec = @import("std/nistec");
const hash = @import("std/hash");
const bytes = @import("std/bytes");

/// RFC 6979 appendix A.2.5's key, and its SHA-256 signature over "sample".
const PUB = "0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299";
const SIG = "3046022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";

const SIG_BAD_R = "3046022100efd48b2bacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const SIG_BAD_S = "3046022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c952d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const SIG_R_ZERO = "3026020100022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const SIG_S_ZERO = "3026022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716020100";
const SIG_R_IS_N = "3046022100ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const SIG_S_IS_N = "3046022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551";

/// `r` with one more leading zero than its sign needs.
const SIG_NONMINIMAL = "304702220000efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
/// `r` with the zero its sign needs taken away, so it reads as negative.
const SIG_NEGATIVE = "30450220efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const SIG_TRAILING = "3046022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda800";
const SIG_INDEFINITE = "3080022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda80000";
const SIG_TRUNCATED = "3046022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843a";
const SIG_NOT_A_SEQ = "3146022100efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716022100f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";

/// The published key with its Y coordinate's last byte changed, so it is a
/// well-formed uncompressed point that is not on this curve. The refusal has
/// to come before any arithmetic: this is the invalid-curve attack.
const PUB_OFF_CURVE = "0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462298";

fn main() i64 {
    const pk = hex(PUB);
    const digest = hash.sha256(bytes.of("sample"));

    // Without this line every other line below passes with a verifier that
    // refuses everything.
    if (nistec.ecdsa_verify(p256(), pk, digest, hex(SIG))) {
        print("the published signature is accepted");
    }

    refuse(pk, digest, SIG_BAD_R, "a flipped r refused");
    refuse(pk, digest, SIG_BAD_S, "a flipped s refused");
    refuse(pk, hash.sha256(bytes.of("samplf")), SIG,
           "a signature of another message refused");
    refuse(pk, digest, SIG_R_ZERO, "r of zero refused");
    refuse(pk, digest, SIG_S_ZERO, "s of zero refused");
    refuse(pk, digest, SIG_R_IS_N, "r at the group order refused");
    refuse(pk, digest, SIG_S_IS_N, "s at the group order refused");
    refuse(pk, digest, SIG_NONMINIMAL, "a non-minimal integer refused");
    refuse(pk, digest, SIG_NEGATIVE, "an integer with its sign bit set refused");
    refuse(pk, digest, SIG_TRAILING, "one trailing byte refused");
    refuse(pk, digest, SIG_INDEFINITE, "an indefinite length refused");
    refuse(pk, digest, SIG_TRUNCATED, "a truncated signature refused");
    refuse(pk, digest, SIG_NOT_A_SEQ, "a SET where a SEQUENCE belongs refused");
    refuse(hex(PUB_OFF_CURVE), digest, SIG, "a public key off the curve refused");
    return 0;
}

fn refuse(pk: []u8, digest: []u8, sig: str, note: str) void {
    if (!nistec.ecdsa_verify(p256(), pk, digest, hex(sig))) { print(note); }
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }

/// The curve, named so the helpers below need no extra parameter.
fn p256() nistec.Curve { return nistec.p256(); }
