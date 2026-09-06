// X.509: the public keys a certificate carries, and verifying with them.
//
// This module is below `std/tls` rather than beside it, and the direction is
// forced. A TLS client has to check a CertificateVerify signature with the key
// out of a certificate, so one of the two modules has to name the other's
// types; and W# has no re-export, so the one that owns `SigKey` is the one
// everything else imports. A certificate is where a public key comes from, so
// that is here.
//
// **A key is a lattice, not a tag.** `RsaKey`, `EcdsaP256Key` and `Ed25519Key`
// are subtypes of `SigKey`, and `verify_signature` is an overload set over
// them -- so the dispatcher picks by the type id the key was born with, in one
// subtract and one compare, and adding a fourth algorithm is adding a struct
// and a function rather than editing a chain. This is the third place in the
// tree that shape has turned up, after the status types and the broker's
// subscribers.
//
// **A signature scheme is a number and stays one.** TLS's `SignatureScheme`
// registry names a key type *and* a hash together -- `rsa_pss_rsae_sha256` is
// one value -- so it is a poor lattice and a good integer. It picks the hash;
// the key's type picks the algorithm.
const array = @import("std/array");
const bytes = @import("std/bytes");
const der = @import("std/der");
const hash = @import("std/hash");
const rsa = @import("std/rsa");
const p256 = @import("std/p256");
const ed = @import("std/curve25519");

// ---------------------------------------------------------------------------
// Signature schemes
// ---------------------------------------------------------------------------
//
// The values TLS 1.3 puts in `signature_algorithms` and in a CertificateVerify.
// The PKCS#1 v1.5 ones are here because a *certificate* may still be signed
// that way even though a CertificateVerify may not be -- RFC 8446 section
// 4.2.3 says exactly that, and a client that refused them could not verify
// most of the chains on the internet.

pub const RSA_PKCS1_SHA256 = 0x0401;
pub const RSA_PKCS1_SHA384 = 0x0501;
pub const RSA_PKCS1_SHA512 = 0x0601;
pub const ECDSA_SECP256R1_SHA256 = 0x0403;
pub const RSA_PSS_RSAE_SHA256 = 0x0804;
pub const RSA_PSS_RSAE_SHA384 = 0x0805;
pub const RSA_PSS_RSAE_SHA512 = 0x0806;
pub const ED25519 = 0x0807;
pub const RSA_PSS_PSS_SHA256 = 0x0809;
pub const RSA_PSS_PSS_SHA384 = 0x080a;
pub const RSA_PSS_PSS_SHA512 = 0x080b;

/// The hash a scheme names, or null when this library does not do that scheme.
///
/// One place decides, so a scheme accepted in a `signature_algorithms` list
/// and a scheme accepted in a CertificateVerify cannot drift apart.
pub fn scheme_hash(scheme: i64) ?hash.Hash {
    if (scheme == RSA_PKCS1_SHA256 or scheme == ECDSA_SECP256R1_SHA256
        or scheme == RSA_PSS_RSAE_SHA256 or scheme == RSA_PSS_PSS_SHA256) {
        return hash.sha256_hash();
    }
    if (scheme == RSA_PKCS1_SHA384 or scheme == RSA_PSS_RSAE_SHA384
        or scheme == RSA_PSS_PSS_SHA384) {
        return hash.sha384_hash();
    }
    if (scheme == RSA_PKCS1_SHA512 or scheme == RSA_PSS_RSAE_SHA512
        or scheme == RSA_PSS_PSS_SHA512) {
        return hash.sha512_hash();
    }
    // Ed25519 hashes internally and names no hash of its own.
    if (scheme == ED25519) { return hash.sha512_hash(); }
    return null;
}

fn is_pss(scheme: i64) bool {
    return scheme == RSA_PSS_RSAE_SHA256 or scheme == RSA_PSS_RSAE_SHA384
        or scheme == RSA_PSS_RSAE_SHA512 or scheme == RSA_PSS_PSS_SHA256
        or scheme == RSA_PSS_PSS_SHA384 or scheme == RSA_PSS_PSS_SHA512;
}

fn is_pkcs1(scheme: i64) bool {
    return scheme == RSA_PKCS1_SHA256 or scheme == RSA_PKCS1_SHA384
        or scheme == RSA_PKCS1_SHA512;
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// A public key of some algorithm. Never instantiated on its own: it is the
/// top of the lattice, and the thing a caller holds when it does not care.
pub const SigKey = struct { };

pub const RsaKey = struct : SigKey { k: rsa.PublicKey };
/// An uncompressed point, 65 bytes, as `std/p256` wants it.
pub const EcdsaP256Key = struct : SigKey { point: []u8 };
pub const Ed25519Key = struct : SigKey { pk: []u8 };

// The algorithm identifiers a SubjectPublicKeyInfo can carry, as the contents
// of their OBJECT IDENTIFIERs.
const OID_RSA = []u8{ 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01 };
const OID_EC = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01 };
const OID_P256 = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07 };
const OID_ED25519 = []u8{ 0x2b, 0x65, 0x70 };

/// The public key a DER SubjectPublicKeyInfo carries.
///
/// ```
/// SubjectPublicKeyInfo ::= SEQUENCE {
///     algorithm         AlgorithmIdentifier,
///     subjectPublicKey  BIT STRING }
/// ```
///
/// An algorithm this library cannot verify with is `error.BadKey` here rather
/// than a key that refuses everything later, because "we do not do that curve"
/// and "the signature is wrong" are different things to a caller.
pub fn parse_spki(spki: []u8) !SigKey {
    const r = der.reader(spki);
    const seq = try der.read_seq(r);
    try der.expect_end(r);
    const alg = try der.read_seq(seq);
    const oid = try der.read_oid(alg);
    const bits = try der.read_bitstring(seq);
    try der.expect_end(seq);

    if (bytes.equal(oid, OID_ED25519)) {
        // RFC 8410: the parameters field is absent, not NULL.
        try der.expect_end(alg);
        if (array.len(bits) != 32) { return error.BadKey; }
        // Annotated on the way out because a coercion into a supertype and
        // one into an error union are not composed: `return Sub{..}` from a
        // function returning `!Base` is a type error, and this is the
        // spelling that says which widening happens first.
        const key: SigKey = Ed25519Key{ .pk = bits };
        return key;
    }
    if (bytes.equal(oid, OID_EC)) {
        const curve = try der.read_oid(alg);
        try der.expect_end(alg);
        // P-256 is the only curve here, because it is the only one with point
        // arithmetic behind it.
        if (!bytes.equal(curve, OID_P256)) { return error.BadKey; }
        if (array.len(bits) != 65 or bits[0] != 4) { return error.BadKey; }
        if (!p256.valid(bits)) { return error.BadKey; }
        const key: SigKey = EcdsaP256Key{ .point = bits };
        return key;
    }
    if (bytes.equal(oid, OID_RSA)) {
        // RFC 3279: the parameters must be present and NULL.
        try der.read_null(alg);
        try der.expect_end(alg);
        const inner = der.reader(bits);
        const key = try der.read_seq(inner);
        try der.expect_end(inner);
        const n = try der.read_uint_bytes(key);
        const e = try der.read_uint_bytes(key);
        try der.expect_end(key);
        const key: SigKey = RsaKey{ .k = try rsa.public_key(n, e) };
        return key;
    }
    return error.BadKey;
}

// ---------------------------------------------------------------------------
// Verifying
// ---------------------------------------------------------------------------
//
// `bool` rather than `!void`, for the reason `ed25519_verify` gives: there is
// one thing a caller does about a refusal, and no information in which refusal
// it was. The overloads below are the whole of the algorithm dispatch.

/// A key whose type this library does not know is a key that verifies nothing.
///
/// Reachable only if a `SigKey` is built that is neither of the three below,
/// which nothing here does -- it is the base of the lattice, and the dispatcher
/// needs a case for it.
pub fn verify_signature(k: SigKey, scheme: i64, content: []u8, sig: []u8) bool {
    return false;
}

pub fn verify_signature(k: RsaKey, scheme: i64, content: []u8, sig: []u8) bool {
    const h = scheme_hash(scheme) orelse return false;
    if (is_pss(scheme)) {
        // TLS 1.3 and modern certificates both use a salt as long as the
        // digest, which is what RFC 8446 section 4.2.3 requires.
        rsa.verify_pss(k.k, h, content, sig, h.digest_size) catch return false;
        return true;
    }
    if (is_pkcs1(scheme)) {
        rsa.verify_pkcs1(k.k, h, content, sig) catch return false;
        return true;
    }
    return false;
}

pub fn verify_signature(k: EcdsaP256Key, scheme: i64, content: []u8, sig: []u8) bool {
    if (scheme != ECDSA_SECP256R1_SHA256) { return false; }
    return p256.ecdsa_verify(k.point, hash.sha256(content), sig);
}

pub fn verify_signature(k: Ed25519Key, scheme: i64, content: []u8, sig: []u8) bool {
    if (scheme != ED25519) { return false; }
    return ed.ed25519_verify(k.pk, content, sig);
}
