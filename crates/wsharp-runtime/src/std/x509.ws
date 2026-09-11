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
const nistec = @import("std/nistec");
const ed = @import("std/curve25519");
const list = @import("std/list");
const text = @import("std/str");
const io = @import("std/io");
const crypto = @import("std/crypto");

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
pub const ECDSA_SECP384R1_SHA384 = 0x0503;
/// Named so that a certificate signed `ecdsa-with-SHA512` by a P-256 or P-384
/// key can be verified. There is no P-521 curve here, so this never names one.
pub const ECDSA_SECP521R1_SHA512 = 0x0603;
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
        or scheme == RSA_PSS_PSS_SHA384 or scheme == ECDSA_SECP384R1_SHA384) {
        return hash.sha384_hash();
    }
    if (scheme == RSA_PKCS1_SHA512 or scheme == RSA_PSS_RSAE_SHA512
        or scheme == RSA_PSS_PSS_SHA512 or scheme == ECDSA_SECP521R1_SHA512) {
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

/// Whether a scheme is ECDSA over *some* curve.
///
/// The curve is the key's, not the scheme's, and that is not a shortcut: in
/// X.509 the algorithm identifier names only the hash -- `ecdsa-with-SHA384`
/// says nothing about which curve signed -- so a P-256 key signing with
/// SHA-384 is an ordinary and legal certificate. TLS's `SignatureScheme`
/// registry conflates the two, and a peer that names the wrong one simply
/// fails to verify, which is the same answer a stricter check would give.
fn is_ecdsa(scheme: i64) bool {
    return scheme == ECDSA_SECP256R1_SHA256 or scheme == ECDSA_SECP384R1_SHA384
        or scheme == ECDSA_SECP521R1_SHA512;
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// A public key of some algorithm. Never instantiated on its own: it is the
/// top of the lattice, and the thing a caller holds when it does not care.
pub const SigKey = struct { };

pub const RsaKey = struct : SigKey { k: rsa.PublicKey };
/// An uncompressed point, as `std/nistec` wants it: 65 bytes on P-256 and 97
/// on P-384. Two types rather than one carrying a curve, so that the
/// dispatcher goes on doing the work -- a third curve is a struct and a
/// function, not an edit to a chain.
pub const EcdsaP256Key = struct : SigKey { point: []u8 };
pub const EcdsaP384Key = struct : SigKey { point: []u8 };
pub const Ed25519Key = struct : SigKey { pk: []u8 };

// The algorithm identifiers a SubjectPublicKeyInfo can carry, as the contents
// of their OBJECT IDENTIFIERs.
const OID_RSA = []u8{ 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01 };
const OID_EC = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01 };
const OID_P256 = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07 };
const OID_P384 = []u8{ 0x2b, 0x81, 0x04, 0x00, 0x22 };
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
        const named = try der.read_oid(alg);
        try der.expect_end(alg);
        // P-256 and P-384 are the curves with point arithmetic behind them.
        // P-521 is not, and a key on it is refused here rather than accepted
        // and then unable to verify anything.
        if (bytes.equal(named, OID_P256)) {
            if (array.len(bits) != 65 or bits[0] != 4) { return error.BadKey; }
            if (!nistec.valid(nistec.p256(), bits)) { return error.BadKey; }
            const key: SigKey = EcdsaP256Key{ .point = bits };
            return key;
        }
        if (bytes.equal(named, OID_P384)) {
            if (array.len(bits) != 97 or bits[0] != 4) { return error.BadKey; }
            if (!nistec.valid(nistec.p384(), bits)) { return error.BadKey; }
            const key: SigKey = EcdsaP384Key{ .point = bits };
            return key;
        }
        return error.BadKey;
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
    if (!is_ecdsa(scheme)) { return false; }
    const h = scheme_hash(scheme) orelse return false;
    return nistec.ecdsa_verify(nistec.p256(), k.point, h.digest(content), sig);
}

pub fn verify_signature(k: EcdsaP384Key, scheme: i64, content: []u8, sig: []u8) bool {
    if (!is_ecdsa(scheme)) { return false; }
    const h = scheme_hash(scheme) orelse return false;
    return nistec.ecdsa_verify(nistec.p384(), k.point, h.digest(content), sig);
}

pub fn verify_signature(k: Ed25519Key, scheme: i64, content: []u8, sig: []u8) bool {
    if (scheme != ED25519) { return false; }
    return ed.ed25519_verify(k.pk, content, sig);
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------
//
// Enough of X.509 to decide whether to talk to somebody, and no more. What is
// read is what is used: the name, the validity, the key, the signature and the
// four extensions that change the answer. Everything else is skipped -- unless
// it is marked critical, which is the issuer saying "refuse this certificate
// rather than ignore me", and refusing is exactly what happens.
//
// **A name is compared as bytes.** RFC 5280 has rules about folding case in a
// PrintableString and normalising whitespace, and no certificate authority
// relies on them: an issuer's name in a certificate and the same authority's
// subject name in its own are the same encoding, because one was copied from
// the other. Comparing the encodings is what every implementation does in
// practice, and it cannot accidentally make two different names equal.

/// The signature algorithms a certificate can be signed with, as the contents
/// of their OBJECT IDENTIFIERs.
const OID_SHA256_RSA = []u8{ 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b };
const OID_SHA384_RSA = []u8{ 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c };
const OID_SHA512_RSA = []u8{ 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d };
const OID_ECDSA_SHA256 = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02 };
const OID_ECDSA_SHA384 = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03 };
const OID_ECDSA_SHA512 = []u8{ 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04 };

/// The extensions this reads. Anything else marked critical is a refusal.
const OID_BASIC_CONSTRAINTS = []u8{ 0x55, 0x1d, 0x13 };
const OID_KEY_USAGE = []u8{ 0x55, 0x1d, 0x0f };
const OID_SUBJECT_ALT_NAME = []u8{ 0x55, 0x1d, 0x11 };
const OID_EXT_KEY_USAGE = []u8{ 0x55, 0x1d, 0x25 };
/// id-kp-serverAuth, the one purpose that matters here.
const OID_SERVER_AUTH = []u8{ 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01 };
/// anyExtendedKeyUsage, which means "every purpose".
const OID_ANY_EKU = []u8{ 0x55, 0x1d, 0x25, 0x00 };

/// `keyUsage` bit 0: the key may sign. A leaf in TLS 1.3 always does.
const KU_DIGITAL_SIGNATURE = 0;
/// `keyUsage` bit 5: the key may sign certificates.
const KU_KEY_CERT_SIGN = 5;

pub const Cert = struct {
    /// The whole encoding, and the range of it the signature covers.
    der: []u8,
    tbs: []u8,
    serial: []u8,
    /// The encoded `Name`s, for comparing an issuer against a subject.
    issuer: []u8,
    subject: []u8,
    not_before: i64,
    not_after: i64,
    key: SigKey,
    /// The scheme the *issuer* signed this certificate with.
    sig_scheme: i64,
    sig: []u8,
    is_ca: bool,
    /// How many intermediates may come below this one, or -1 for no limit.
    path_len: i64,
    /// True when there is a `keyUsage` and it permits the thing named.
    can_sign: bool,
    can_sign_certs: bool,
    /// True when there is no `extKeyUsage`, or it names server authentication.
    for_server_auth: bool,
    dns_names: list.List[str],
};

fn scheme_of_oid(oid: []u8) i64 {
    if (bytes.equal(oid, OID_SHA256_RSA)) { return RSA_PKCS1_SHA256; }
    if (bytes.equal(oid, OID_SHA384_RSA)) { return RSA_PKCS1_SHA384; }
    if (bytes.equal(oid, OID_SHA512_RSA)) { return RSA_PKCS1_SHA512; }
    if (bytes.equal(oid, OID_ECDSA_SHA256)) { return ECDSA_SECP256R1_SHA256; }
    if (bytes.equal(oid, OID_ECDSA_SHA384)) { return ECDSA_SECP384R1_SHA384; }
    if (bytes.equal(oid, OID_ECDSA_SHA512)) { return ECDSA_SECP521R1_SHA512; }
    if (bytes.equal(oid, OID_ED25519)) { return ED25519; }
    return 0;
}

/// The algorithm identifier's OID, with whatever parameters follow skipped.
///
/// The OID rather than the scheme it maps to, because the two copies of this
/// field have to be compared and two *different* algorithms this library does
/// not know would both map to "unknown" and compare equal.
fn read_alg_oid(r: der.Reader) ![]u8 {
    const alg = try der.read_seq(r);
    const oid = try der.read_oid(alg);
    // The parameters are NULL for the RSA algorithms and absent for the rest;
    // either way nothing here reads them.
    while (!der.at_end(alg)) { try der.skip_value(alg); }
    return oid;
}

/// A certificate, read from its encoding.
pub fn parse(raw: []u8) !Cert {
    const outer = der.reader(raw);
    const top = try der.read_seq(outer);
    try der.expect_end(outer);

    // The TBSCertificate is taken whole, because that is what the signature
    // covers -- a re-encoding of the fields would be a different byte string.
    const tbs_value = try der.read_value(top);
    if (tbs_value.tag != der.SEQUENCE) { return error.BadCertificate; }
    const outer_oid = try read_alg_oid(top);
    const sig = try der.read_bitstring(top);
    try der.expect_end(top);

    const tbs = der.body(tbs_value);
    // v3, written down. A version 1 certificate carries no extensions, so it
    // can say nothing about being a CA -- and one used as one is the oldest
    // hole in this format.
    if (!der.has_tag(tbs, der.context(0))) { return error.BadCertificate; }
    const ver = try der.read_tagged(tbs, der.context(0));
    if ((try der.read_uint(ver)) != 2) { return error.BadCertificate; }
    try der.expect_end(ver);

    const serial = try der.read_int_bytes(tbs);
    const inner_oid = try read_alg_oid(tbs);
    // RFC 5280 section 4.1.1.2: the two must agree. They are signed and
    // unsigned copies of the same field, so a mismatch is somebody editing one.
    if (!bytes.equal(inner_oid, outer_oid)) { return error.BadCertificate; }
    // An algorithm this library cannot verify is *not* a refusal here, and
    // that is deliberate. A trust anchor's own signature is never checked --
    // it is trusted for being in the store, not for having signed itself --
    // so refusing one would drop authorities for a reason that never applies
    // to them. The scheme becomes zero instead, which `verify_signature`
    // refuses, so a chain that actually needs the signature still fails.
    const scheme = scheme_of_oid(outer_oid);

    const issuer = der.encoded(try der.read_value(tbs));
    const validity = try der.read_seq(tbs);
    const not_before = try der.read_time(validity);
    const not_after = try der.read_time(validity);
    try der.expect_end(validity);
    const subject = der.encoded(try der.read_value(tbs));
    const key = try parse_spki(der.encoded(try der.read_value(tbs)));

    var is_ca = false;
    var path_len = -1;
    var can_sign = true;
    var can_sign_certs = true;
    var for_server_auth = true;
    var names: list.List[str] = list.new();

    // The unique identifiers, which nothing has used since 1988.
    if (der.has_tag(tbs, der.context_prim(1))) { try der.skip_value(tbs); }
    if (der.has_tag(tbs, der.context_prim(2))) { try der.skip_value(tbs); }

    if (der.has_tag(tbs, der.context(3))) {
        const wrapper = try der.read_tagged(tbs, der.context(3));
        const exts = try der.read_seq(wrapper);
        try der.expect_end(wrapper);
        while (!der.at_end(exts)) {
            const ext = try der.read_seq(exts);
            const oid = try der.read_oid(ext);
            var critical = false;
            if (der.has_tag(ext, der.BOOLEAN)) { critical = try der.read_bool(ext); }
            const value = try der.read_octets(ext);
            try der.expect_end(ext);
            const inner = der.reader(value);

            if (bytes.equal(oid, OID_BASIC_CONSTRAINTS)) {
                const bc = try der.read_seq(inner);
                if (der.has_tag(bc, der.BOOLEAN)) { is_ca = try der.read_bool(bc); }
                if (!der.at_end(bc)) { path_len = try der.read_uint(bc); }
                try der.expect_end(bc);
            } else if (bytes.equal(oid, OID_KEY_USAGE)) {
                const bits = try der.read_bits(inner);
                can_sign = der.bit_set(bits, KU_DIGITAL_SIGNATURE);
                can_sign_certs = der.bit_set(bits, KU_KEY_CERT_SIGN);
            } else if (bytes.equal(oid, OID_SUBJECT_ALT_NAME)) {
                const san = try der.read_seq(inner);
                while (!der.at_end(san)) {
                    const gn = try der.read_value(san);
                    // dNSName is `[2] IMPLICIT IA5String`. Every other kind of
                    // GeneralName -- an address, an email, a URI -- says
                    // nothing about which host this is.
                    if (gn.tag == der.context_prim(2)) {
                        list.push(names, bytes.to_str(der.raw(gn)));
                    }
                }
            } else if (bytes.equal(oid, OID_EXT_KEY_USAGE)) {
                const eku = try der.read_seq(inner);
                for_server_auth = false;
                while (!der.at_end(eku)) {
                    const purpose = try der.read_oid(eku);
                    if (bytes.equal(purpose, OID_SERVER_AUTH)
                        or bytes.equal(purpose, OID_ANY_EKU)) {
                        for_server_auth = true;
                    }
                }
            } else {
                // The issuer said this extension changes the meaning of the
                // certificate and this code does not know how. Ignoring it is
                // exactly what "critical" forbids.
                if (critical) { return error.UnsupportedExtension; }
            }
        }
    }
    try der.expect_end(tbs);

    return Cert{
        .der = raw, .tbs = der.encoded(tbs_value), .serial = serial,
        .issuer = issuer, .subject = subject,
        .not_before = not_before, .not_after = not_after,
        .key = key, .sig_scheme = scheme, .sig = sig,
        .is_ca = is_ca, .path_len = path_len,
        .can_sign = can_sign, .can_sign_certs = can_sign_certs,
        .for_server_auth = for_server_auth, .dns_names = names,
    };
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------
//
// **The subject alternative name, and nothing else.** RFC 6125 and every
// browser stopped looking at the common name years ago, and the reason is
// worth keeping: a CN is a display string with no structure, so a certificate
// for `CN=example.com, O=Some Company` and one for a company literally named
// `example.com` are the same bytes to a naive reader. A `dNSName` says what it
// is.

/// Whether `host` ends with `suffix`.
fn ends_with(s: str, suffix: str) bool {
    const n = text.len(s);
    const m = text.len(suffix);
    if (m > n) { return false; }
    return text.substr(s, n - m, n) == suffix;
}

/// Whether a `dNSName` from a certificate covers `host`.
///
/// One wildcard, and only as the whole of the leftmost label: `*.a.com`
/// matches `b.a.com` and does not match `a.com` -- a certificate for a
/// subdomain is not a certificate for the domain -- and does not match
/// `c.b.a.com`, because a wildcard covers one label and not a tree.
/// `w*.a.com` is not a wildcard at all here, which is what RFC 6125 section
/// 6.4.3 recommends and what avoids arguing about where the star may sit.
fn covers(pattern: str, host: str) bool {
    if (pattern == host) { return pattern != ""; }
    if (!text.starts_with(pattern, "*.")) { return false; }
    // ".a.com" -- the dot is kept, so the host must have a label before it.
    const suffix = text.substr(pattern, 1, text.len(pattern));
    if (!ends_with(host, suffix)) { return false; }
    const head = text.len(host) - text.len(suffix);
    if (head < 1) { return false; }
    var i = 0;
    while (i < head) : (i += 1) {
        if (text.byte_at(host, i) == 46) { return false; }
    }
    return true;
}

/// Whether the certificate was issued for this host.
pub fn matches_host(c: Cert, host: str) bool {
    const want = text.to_lower(host);
    const it = list.iter(c.dns_names);
    while (list.next(it)) |name| {
        if (covers(text.to_lower(name), want)) { return true; }
    }
    return false;
}

// ---------------------------------------------------------------------------
// Chains
// ---------------------------------------------------------------------------

/// How many certificates a chain may have between the leaf and an anchor.
///
/// Not a rule from anywhere: it is a bound, so that a pool containing a cycle
/// -- two certificates that name each other as issuer -- is a refusal rather
/// than a loop.
const MAX_CHAIN = 8;

fn valid_at(c: Cert, now: i64) bool {
    return now >= c.not_before and now <= c.not_after;
}

/// An issuer of `child` among `pool` whose signature over it checks out.
///
/// Everything is verified before the answer is given, so a certificate that
/// merely has the right name cannot displace one that also has the right key
/// -- which is what makes a pool with a name collision in it safe.
fn issuer_in(pool: list.List[Cert], child: Cert, now: i64, ca: bool) ?Cert {
    const it = list.iter(pool);
    while (list.next(it)) |c| {
        if (!bytes.equal(c.subject, child.issuer)) { continue; }
        if (ca and (!c.is_ca or !c.can_sign_certs)) { continue; }
        if (!valid_at(c, now)) { continue; }
        if (!verify_signature(c.key, child.sig_scheme, child.tbs, child.sig)) { continue; }
        return c;
    }
    return null;
}

/// Parse what parses, and quietly drop what does not.
///
/// A store is a bag of certificates and one this library cannot read -- an
/// algorithm it does not do, a critical extension it does not know -- is a
/// reason to trust one fewer authority, not a reason to refuse every
/// connection. A *chain* is different, and `verify_chain` reports there.
pub fn parse_all(ders: [][]u8) list.List[Cert] {
    var out: list.List[Cert] = list.new();
    var i = 0;
    const n = array.len(ders);
    while (i < n) : (i += 1) {
        const c = parse(ders[i]) catch continue;
        list.push(out, c);
    }
    return out;
}

/// What a verification found, and not only that it succeeded.
///
/// `verify_chain` answers the question a handshake asks -- may this key be
/// trusted -- and for a connection that is the whole of it. Something that
/// wants to *show* the verification needs the two facts the walk knew and threw
/// away: which anchor ended it, and how many of the certificates the peer sent
/// were used getting there. Nothing here is a new check. It is the same walk,
/// reporting.
pub const Path = struct {
    /// The leaf's public key: what `verify_chain` hands back.
    key: SigKey,
    /// The certificate in `roots` the walk reached.
    anchor: Cert,
    /// How many of the peer's intermediates sat between the leaf and the
    /// anchor. Zero when the anchor signed the leaf itself.
    depth: i64,
};

/// The leaf's public key, if the chain from it reaches a trusted anchor.
///
/// `intermediates` are the certificates the peer sent below its leaf, and are
/// trusted for nothing: each is checked to be a CA, to be valid now, and to
/// have signed the one below it. `roots` are the anchors, and reaching one is
/// what ends the walk -- an anchor's own signature is never checked, because a
/// self-signed certificate proves only that whoever made it had its key, and
/// the trust comes from it being in the store.
pub fn verify_chain(leaf_der: []u8, intermediates: [][]u8, roots: list.List[Cert],
                    host: str, now: i64) !SigKey {
    const found = try verify_path(leaf_der, intermediates, roots, host, now);
    return found.key;
}

/// The same walk, saying where it ended.
///
/// Separate from `verify_chain` rather than replacing it, because a caller that
/// only needs the key should not have to name a type to ignore two thirds of
/// it -- and because one implementation of the walk is the point. `verify_chain`
/// is this function with the answer narrowed.
pub fn verify_path(leaf_der: []u8, intermediates: [][]u8, roots: list.List[Cert],
                   host: str, now: i64) !Path {
    const leaf = try parse(leaf_der);
    if (now < leaf.not_before) { return error.CertificateNotYetValid; }
    if (now > leaf.not_after) { return error.CertificateExpired; }
    // A leaf that says it may not sign cannot be the one on the other end of a
    // TLS 1.3 handshake, which is a signature over the transcript.
    if (!leaf.can_sign) { return error.BadCertificate; }
    if (!leaf.for_server_auth) { return error.BadCertificate; }
    if (host != "" and !matches_host(leaf, host)) { return error.NameMismatch; }

    const middle = parse_all(intermediates);
    var current = leaf;
    var depth = 0;
    while (depth < MAX_CHAIN) : (depth += 1) {
        if (issuer_in(roots, current, now, true)) |anchor| {
            return Path{ .key = leaf.key, .anchor = anchor, .depth = depth };
        }
        const next = issuer_in(middle, current, now, true) orelse {
            return error.UnknownIssuer;
        };
        // `pathLenConstraint` is how many intermediates may sit below this one.
        // At `depth` there are exactly that many between it and the leaf.
        if (next.path_len >= 0 and next.path_len < depth) {
            return error.BadCertificate;
        }
        // A certificate that is its own issuer and is not in the store is not
        // an anchor; stopping here turns a cycle into a refusal now rather
        // than at the depth bound.
        if (bytes.equal(next.subject, next.issuer)) { return error.UnknownIssuer; }
        current = next;
    }
    return error.ChainTooLong;
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

const PEM_BEGIN = "-----BEGIN CERTIFICATE-----";
const PEM_END = "-----END CERTIFICATE-----";

/// Every certificate a PEM file holds.
///
/// Anything that is not a `CERTIFICATE` block is skipped, and so is a block
/// whose base64 does not decode or whose contents this library cannot read --
/// for `parse_all`'s reason. A bundle is a list of authorities, not a
/// structure.
pub fn pem_certificates(pem: str) list.List[Cert] {
    var out: list.List[Cert] = list.new();
    var rest = pem;
    var guard = 0;
    while (guard < 4096) : (guard += 1) {
        const begin = text.find(rest, PEM_BEGIN);
        if (begin < 0) { return out; }
        const body_at = begin + text.len(PEM_BEGIN);
        const tail = text.substr(rest, body_at, text.len(rest));
        const end = text.find(tail, PEM_END);
        if (end < 0) { return out; }
        const der = bytes.from_base64(text.substr(tail, 0, end)) catch bytes.new(0);
        if (array.len(der) > 0) {
            const c = parse(der) catch bad_cert();
            if (array.len(c.der) > 0) { list.push(out, c); }
        }
        rest = text.substr(tail, end + text.len(PEM_END), text.len(tail));
    }
    return out;
}

/// A stand-in for a certificate that did not parse.
///
/// W# has no way to say "skip this one" from inside a `catch` that has to
/// produce a value, so the value produced is one with an empty encoding, which
/// the caller drops. Ugly, and honest about being so.
fn bad_cert() Cert {
    var names: list.List[str] = list.new();
    const empty = bytes.new(0);
    return Cert{
        .der = empty, .tbs = empty, .serial = empty,
        .issuer = empty, .subject = empty,
        .not_before = 0, .not_after = 0, .key = SigKey{},
        .sig_scheme = 0, .sig = empty,
        .is_ca = false, .path_len = -1,
        .can_sign = false, .can_sign_certs = false, .for_server_auth = false,
        .dns_names = names,
    };
}

/// The blob the platform store hands over: repeated four-byte length and DER.
fn split_blob(blob: []u8) list.List[Cert] {
    var out: list.List[Cert] = list.new();
    var at = 0;
    const n = array.len(blob);
    while (at + 4 <= n) {
        const len = i64(bytes.be32(blob, at));
        at += 4;
        if (len < 0 or at + len > n) { return out; }
        const c = parse(bytes.slice(blob, at, at + len)) catch bad_cert();
        if (array.len(c.der) > 0) { list.push(out, c); }
        at += len;
    }
    return out;
}

/// The paths a Unix system keeps its certificate bundle at.
///
/// An if-chain rather than a table because a top-level `const` array may hold
/// only scalars, and these are strings. `http.status_of` is the same shape for
/// the same reason.
fn bundle_path(i: i64) str {
    if (i == 0) { return "/etc/ssl/certs/ca-certificates.crt"; }   // Debian, NixOS
    if (i == 1) { return "/etc/pki/tls/certs/ca-bundle.crt"; }     // Fedora, RHEL
    if (i == 2) { return "/etc/ssl/ca-bundle.pem"; }               // openSUSE
    if (i == 3) { return "/etc/ssl/cert.pem"; }                    // Alpine, FreeBSD
    if (i == 4) { return "/usr/local/share/certs/ca-root-nss.crt"; }
    if (i == 5) { return "/etc/ssl/certs/ca-bundle.crt"; }
    return "";
}

/// The certificates this machine trusts.
///
/// The platform store first -- macOS keeps its anchors in a keychain and
/// Windows in a store API, and neither is a file -- and then the list of
/// candidate paths, which is every Linux and every BSD. `error.NoRootStore`
/// when nothing answered, which is a real thing to report: a container built
/// without a bundle is the usual cause, and a connection that failed for that
/// reason should say so rather than say the certificate was bad.
pub fn system_roots() !list.List[Cert] {
    const blob = crypto.raw_system_roots() catch "";
    if (text.len(blob) > 0) {
        const platform = split_blob(bytes.of(blob));
        if (list.len(platform) > 0) { return platform; }
    }
    var i = 0;
    while (i < 8) : (i += 1) {
        const path = bundle_path(i);
        if (path == "") { break; }
        if (io.exists(path)) {
            const pem = io.read_file(path) catch continue;
            const found = pem_certificates(pem);
            if (list.len(found) > 0) { return found; }
        }
    }
    return error.NoRootStore;
}
