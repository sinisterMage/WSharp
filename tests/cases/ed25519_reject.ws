// Every way an Ed25519 signature can fail to be one, a line each.
//
// The discipline is the AEADs' and the RSA verifier's: a single "a bad
// signature is refused" check passes with almost any bug present, because the
// interesting failures here are *acceptances*. A verifier that skips the
// reduction check on `S` accepts a second spelling of every signature; one
// that masks a non-canonical `y` accepts a second spelling of every key; and
// both are how one peer comes to believe a message another peer will not.
//
// The base vector is RFC 8032 section 7.1's TEST 3, and every mutation of it
// below was handed to node's `crypto.verify` before it was written here. All
// thirteen answers -- the acceptance and the twelve refusals -- agree, which
// is the point: this rejects exactly what OpenSSL rejects, so a signature
// this library accepts is one a TLS peer accepts.
// expect: the published signature is accepted
// expect: a flipped signature byte refused
// expect: a flipped message byte refused
// expect: S equal to the group order refused
// expect: S above the group order refused
// expect: a signature one byte short refused
// expect: a signature one byte long refused
// expect: an all-zero signature refused
// expect: a non-canonical R refused
// expect: another signature's R refused
// expect: an all-zero public key refused
// expect: a non-canonical public key refused
// expect: another key's signature refused
// expect: a public key of the wrong length refused
const ed = @import("std/curve25519");
const bytes = @import("std/bytes");

/// RFC 8032 section 7.1, TEST 3.
const PK = "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025";
const MSG = "af82";
const SIG = "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a";

/// One bit of `R`, which makes the equation fail rather than a check in front
/// of it.
const SIG_FLIPPED = "6291d657deed24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a";

/// `S` replaced by `L` and by `L + 1`. Both are congruent to a scalar that
/// would verify, so a verifier that reduces instead of refusing accepts them
/// -- and then one signature has infinitely many spellings.
const SIG_S_IS_L = "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3acedd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010";
const SIG_S_ABOVE_L = "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3aceed3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010";

const SIG_SHORT = "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec4";
const SIG_LONG = "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a00";
const SIG_ZERO = "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

/// `R` encoded as the field prime itself, which is congruent to zero and so is
/// a point -- but is not the encoding of one. Masking the top bit and reducing
/// would accept it.
const SIG_R_NONCANON = "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a";

/// A well-formed `R` from a different signature, with this one's `S`.
const SIG_OTHER_R = "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a";

const PK_ZERO = "0000000000000000000000000000000000000000000000000000000000000000";
const PK_NONCANON = "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f";
const PK_SHORT = "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb9115489080";

/// RFC 8032 section 7.1, TEST 2's key.
const PK_OTHER = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";

fn main() i64 {
    // The half that keeps the other half honest: a verifier that refuses
    // everything passes every line below and fails this one.
    if (ed.ed25519_verify(hex(PK), hex(MSG), hex(SIG))) {
        print("the published signature is accepted");
    }

    refuse(PK, MSG, SIG_FLIPPED, "a flipped signature byte refused");
    refuse(PK, "af83", SIG, "a flipped message byte refused");
    refuse(PK, MSG, SIG_S_IS_L, "S equal to the group order refused");
    refuse(PK, MSG, SIG_S_ABOVE_L, "S above the group order refused");
    refuse(PK, MSG, SIG_SHORT, "a signature one byte short refused");
    refuse(PK, MSG, SIG_LONG, "a signature one byte long refused");
    refuse(PK, MSG, SIG_ZERO, "an all-zero signature refused");
    refuse(PK, MSG, SIG_R_NONCANON, "a non-canonical R refused");
    refuse(PK, MSG, SIG_OTHER_R, "another signature's R refused");
    refuse(PK_ZERO, MSG, SIG, "an all-zero public key refused");
    refuse(PK_NONCANON, MSG, SIG, "a non-canonical public key refused");
    refuse(PK_OTHER, MSG, SIG, "another key's signature refused");
    refuse(PK_SHORT, MSG, SIG, "a public key of the wrong length refused");
    return 0;
}

fn refuse(pk: str, msg: str, sig: str, note: str) void {
    if (!ed.ed25519_verify(hex(pk), hex(msg), hex(sig))) { print(note); }
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
