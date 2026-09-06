// RSA signature verification: PKCS#1 v1.5 and PSS, from RFC 8017.
//
// **Verification only, and that is a decision rather than a stopping point.**
// TLS 1.3 does no RSA key exchange and no RSA key transport, so the only thing
// an RSA key is ever asked to do here is confirm that somebody else signed
// something -- which uses the public exponent, is done on a certificate
// anybody can fetch, and involves no secret at all. Writing the private half
// would be code with no caller in a file where being wrong is a security
// problem, which is the same reason `std/cipher` does not write the AES
// inverse cipher.
//
// Two things follow from that and are worth saying out loud, because they are
// the opposite of what the rest of this library assumes:
//
// - **Nothing here needs to be constant time.** The modulus, the exponent, the
//   signature and the message are all public. `bignum.modexp` says the same
//   thing about itself.
// - **The public exponent is tiny.** 65537 is sixteen squarings and one
//   multiplication, so a verification costs about as much as seventeen
//   multiplications of the modulus by itself, and the Montgomery setup that
//   precedes it costs more than the exponentiation does.
//
// Both schemes are needed and neither is optional: TLS 1.3 signs its
// `CertificateVerify` with PSS, and most of the public internet's certificate
// chains are still signed with PKCS#1 v1.5.
const array = @import("std/array");
const bytes = @import("std/bytes");
const bignum = @import("std/bignum");
const hash = @import("std/hash");

// ---------------------------------------------------------------------------
// DigestInfo
// ---------------------------------------------------------------------------
//
// PKCS#1 v1.5 signs a DER-encoded `DigestInfo`, which is a SEQUENCE of the
// hash's algorithm identifier and the digest as an OCTET STRING. The three
// prefixes below are that structure with the digest cut off, which is the only
// form anything needs: nothing here parses one, it builds the whole encoded
// message and compares.
//
// Each is `30 L 30 0d 06 09 <oid> 05 00 04 <n>` -- an outer SEQUENCE, an
// AlgorithmIdentifier holding the OID and an explicit NULL, and the header of
// the OCTET STRING. The OIDs differ in one byte, which is the algorithm
// number in 2.16.840.1.101.3.4.2.

const DI_SHA256 = []u8{
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86,
    0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
};
const DI_SHA384 = []u8{
    0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86,
    0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05,
    0x00, 0x04, 0x30,
};
const DI_SHA512 = []u8{
    0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86,
    0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05,
    0x00, 0x04, 0x40,
};

/// The prefix for `h`, chosen by its digest size.
///
/// The same trick HMAC uses to be one function rather than three: the hash
/// arrives as a value, so the thing to ask it is a question about the value.
/// A size that is none of the three is a mistake in the caller -- TLS 1.3
/// allows exactly SHA-256, SHA-384 and SHA-512 here.
fn digest_info(h: hash.Hash) []u8 {
    assert(h.digest_size == 32 or h.digest_size == 48 or h.digest_size == 64);
    if (h.digest_size == 32) { return DI_SHA256; }
    if (h.digest_size == 48) { return DI_SHA384; }
    return DI_SHA512;
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

pub const PublicKey = struct {
    /// Arithmetic modulo the modulus.
    m: bignum.Mont,
    /// The public exponent.
    e: []u32,
    /// The modulus in limbs, and in bytes and bits -- `size` is RFC 8017's
    /// `k`, which is what every encoded message here is as long as.
    limbs: i64,
    size: i64,
    bits: i64,
};

/// A public key from its modulus and exponent, each big-endian.
///
/// Leading zero bytes are allowed and ignored, because that is how a DER
/// `INTEGER` writes a value whose top bit is set. What is not allowed is a
/// modulus that is even or absurdly small, or an exponent that is even or one:
/// each of those is a key no signature under it could mean anything, and
/// finding out here is better than finding out in the arithmetic.
pub fn public_key(n: []u8, e: []u8) !PublicKey {
    const limbs = (array.len(n) + 3) / 4;
    if (limbs == 0) { return error.BadKey; }
    // An exponent wider than the modulus would be silently cut down to it,
    // and a key that means something other than what it says is worse than a
    // key that is refused.
    if (array.len(e) > array.len(n)) { return error.BadKey; }
    const nn = bignum.from_be(n, 0, array.len(n), limbs);
    const ee = bignum.from_be(e, 0, array.len(e), limbs);
    const bits = bignum.bit_len(nn);
    // 512 bits is already far too small to mean anything; this rejects the
    // degenerate cases rather than pretending to set a policy.
    if (bits < 512) { return error.BadKey; }
    if ((nn[0] & 1) == 0) { return error.BadKey; }
    if ((ee[0] & 1) == 0 or bignum.bit_len(ee) < 2) { return error.BadKey; }
    return PublicKey{
        .m = bignum.mont(nn),
        .e = ee,
        .limbs = limbs,
        .size = (bits + 7) / 8,
        .bits = bits,
    };
}

/// `sig^e mod n`, as exactly `size` bytes -- RFC 8017's RSAVP1.
///
/// A signature that is not `size` bytes long, or that is not below the
/// modulus, is refused rather than reduced: both are the encoding of some
/// other number, and accepting them would make one message have many valid
/// signatures.
fn public_op(k: PublicKey, sig: []u8) ![]u8 {
    if (array.len(sig) != k.size) { return error.BadSignature; }
    const s = bignum.from_be(sig, 0, k.size, k.limbs);
    if (bignum.cmp(s, k.m.n) >= 0) { return error.BadSignature; }
    const out = bignum.new(k.limbs);
    bignum.modexp(k.m, out, s, k.e);
    const em = bytes.new(k.size);
    bignum.to_be(out, em);
    return em;
}

// ---------------------------------------------------------------------------
// PKCS#1 v1.5
// ---------------------------------------------------------------------------

/// Verify a PKCS#1 v1.5 signature over `msg`.
///
/// The encoded message is *built* and compared rather than parsed, which is
/// the difference between this and the family of bugs that let a signature
/// with a short padding run and trailing rubbish through. There is exactly one
/// byte string a valid signature can decrypt to, so producing it and comparing
/// is both the simplest implementation and the strictest one.
pub fn verify_pkcs1(k: PublicKey, h: hash.Hash, msg: []u8, sig: []u8) !void {
    const em = try public_op(k, sig);
    const di = digest_info(h);
    const t_len = array.len(di) + h.digest_size;
    // EM is 0x00 0x01, then at least eight 0xff, then 0x00, then T.
    if (k.size < t_len + 11) { return error.BadSignature; }

    const want = bytes.new(k.size);
    want[1] = 1;
    bytes.fill(want, 2, k.size - t_len - 3, 0xff);
    bytes.copy(want, k.size - t_len, di, 0, array.len(di));
    const digest = h.digest(msg);
    bytes.copy(want, k.size - h.digest_size, digest, 0, h.digest_size);

    if (!bytes.equal(em, want)) { return error.BadSignature; }
    return;
}

// ---------------------------------------------------------------------------
// PSS
// ---------------------------------------------------------------------------

/// MGF1 over `h`: as many bytes as `out` holds, from `seed`.
///
/// One buffer for the seed and the counter, reused per block, so a mask of any
/// length allocates the digests and nothing else.
fn mgf1(h: hash.Hash, seed: []u8, out: []u8) void {
    const seed_len = array.len(seed);
    const n = array.len(out);
    const block = bytes.new(seed_len + 4);
    bytes.copy(block, 0, seed, 0, seed_len);
    var done = 0;
    var counter: u32 = 0;
    while (done < n) {
        bytes.put_be32(block, seed_len, counter);
        const d = h.digest(block);
        var take = h.digest_size;
        if (done + take > n) { take = n - done; }
        bytes.copy(out, done, d, 0, take);
        done += take;
        counter += 1;
    }
    return;
}

/// Verify an RSASSA-PSS signature over `msg`, with a salt of `salt_len` bytes.
///
/// EMSA-PSS-VERIFY, RFC 8017 section 9.1.2, step for step. The salt length is
/// the caller's to state because it is not recoverable from the signature
/// without trusting it: TLS 1.3 fixes it at the digest length, and a
/// certificate carries it in the signature algorithm's parameters.
pub fn verify_pss(k: PublicKey, h: hash.Hash, msg: []u8, sig: []u8, salt_len: i64) !void {
    // A negative length is a mistake in the caller; every other value is
    // checked against the encoded message below.
    assert(salt_len >= 0);
    const full = try public_op(k, sig);
    const h_len = h.digest_size;
    // The encoded message is one bit shorter than the modulus, which is a byte
    // shorter when the modulus is a whole number of bytes plus one bit.
    const em_bits = k.bits - 1;
    const em_len = (em_bits + 7) / 8;
    if (em_len < h_len + salt_len + 2) { return error.BadSignature; }
    const skip = k.size - em_len;
    var i = 0;
    while (i < skip) : (i += 1) {
        if (full[i] != 0) { return error.BadSignature; }
    }
    const em = bytes.slice(full, skip, skip + em_len);
    if (em[em_len - 1] != 0xbc) { return error.BadSignature; }

    const db_len = em_len - h_len - 1;
    const seed = bytes.slice(em, db_len, db_len + h_len);

    // The bits the encoded message is shorter than its bytes must be zero, in
    // the masked form and again after unmasking. Written with an `if` rather
    // than as `0xff << (8 - spare)` because a shift amount is masked to the
    // operand's width, so a shift by eight of a `u8` is the identity and the
    // whole byte would be required to be zero.
    const spare = 8 * em_len - em_bits;
    var top: u8 = 0;
    if (spare > 0) { top = u8(0xff) << u8(8 - spare); }
    if ((em[0] & top) != 0) { return error.BadSignature; }

    const mask = bytes.new(db_len);
    mgf1(h, seed, mask);
    const db = bytes.new(db_len);
    i = 0;
    while (i < db_len) : (i += 1) { db[i] = em[i] ^ mask[i]; }
    db[0] &= ~top;

    // DB is zeros, then a single 0x01, then the salt.
    i = 0;
    while (i < db_len - salt_len - 1) : (i += 1) {
        if (db[i] != 0) { return error.BadSignature; }
    }
    if (db[db_len - salt_len - 1] != 1) { return error.BadSignature; }

    // M' is eight zero bytes, the message's digest, and the salt -- and its
    // digest is what the signature actually committed to.
    const primed = bytes.new(8 + h_len + salt_len);
    const digest = h.digest(msg);
    bytes.copy(primed, 8, digest, 0, h_len);
    bytes.copy(primed, 8 + h_len, db, db_len - salt_len, salt_len);
    if (!bytes.equal(h.digest(primed), seed)) { return error.BadSignature; }
    return;
}
