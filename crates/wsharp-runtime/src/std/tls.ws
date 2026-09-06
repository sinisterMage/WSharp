// TLS 1.3, and only TLS 1.3 -- RFC 8446.
//
// No 1.2, no fallback, no downgrade dance. A client that cannot talk to a
// 1.2-only server fails loudly against a server that should be upgraded, and
// every hour spent on 1.2 is an hour spent on the version with the worse
// security story. There is no renegotiation, no compression, no session
// resumption and no 0-RTT: each is a feature 1.3 either removed or made
// optional, and the ones it made optional are the ones with the sharp edges.
//
// **The core takes bytes in and hands bytes out; it never touches a socket.**
// `feed` is given whatever arrived, `pending` says what to send. That is not an
// abstraction for its own sake -- it is what lets a whole handshake be driven
// from a test with no I/O at all, which is how the RFC 8448 traces are
// replayed, and it is the only way one thread can be both ends of a
// connection: a blocking `connect` would write its ClientHello and then wait
// for a reply nobody is left to send. `Session` below is the thin blocking
// wrapper for callers that do have a socket.
//
// **Both ends are here**, and they share the record layer, the key schedule and
// the message codecs. A server is not a thing this library expects anyone to
// deploy; it exists because a client with no server to talk to can only be
// tested against recorded bytes, and recorded bytes cannot test the bytes this
// client itself produces.
//
// **Constant time is a construction, not a guarantee**, as everywhere else in
// item 10. What matters here beyond the primitives is that a MAC comparison
// does not stop early -- `bytes.equal` is a builtin that reads every byte --
// and that a decryption failure is one error whatever went wrong inside it.
const array = @import("std/array");
const bytes = @import("std/bytes");
const hash = @import("std/hash");
const cipher = @import("std/cipher");
const crypto = @import("std/crypto");
const curve = @import("std/curve25519");
const nistec = @import("std/nistec");
const x509 = @import("std/x509");
const net = @import("std/net");
const list = @import("std/list");
const time = @import("std/time");

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// Record content types.
const CT_CHANGE_CIPHER_SPEC = 20;
const CT_ALERT = 21;
const CT_HANDSHAKE = 22;
const CT_APPLICATION_DATA = 23;

/// Handshake message types.
const HS_CLIENT_HELLO = 1;
const HS_SERVER_HELLO = 2;
const HS_NEW_SESSION_TICKET = 4;
const HS_ENCRYPTED_EXTENSIONS = 8;
const HS_CERTIFICATE = 11;
const HS_CERTIFICATE_REQUEST = 13;
const HS_CERTIFICATE_VERIFY = 15;
const HS_FINISHED = 20;
const HS_KEY_UPDATE = 24;
/// The synthetic message a HelloRetryRequest replaces the first ClientHello
/// with in the transcript, RFC 8446 section 4.4.1.
const HS_MESSAGE_HASH = 254;

/// Extension types.
const EXT_SERVER_NAME = 0;
const EXT_SUPPORTED_GROUPS = 10;
const EXT_SIGNATURE_ALGORITHMS = 13;
const EXT_SUPPORTED_VERSIONS = 43;
const EXT_COOKIE = 44;
const EXT_KEY_SHARE = 51;

pub const TLS_AES_128_GCM_SHA256 = 0x1301;
pub const TLS_AES_256_GCM_SHA384 = 0x1302;
pub const TLS_CHACHA20_POLY1305_SHA256 = 0x1303;

pub const GROUP_X25519 = 0x001d;
pub const GROUP_SECP256R1 = 0x0017;
pub const GROUP_SECP384R1 = 0x0018;

/// Alert descriptions, for the ones this sends.
const AL_CLOSE_NOTIFY = 0;
const AL_UNEXPECTED_MESSAGE = 10;
const AL_BAD_RECORD_MAC = 20;
const AL_HANDSHAKE_FAILURE = 40;
const AL_RECORD_OVERFLOW = 22;
const AL_BAD_CERTIFICATE = 42;
const AL_ILLEGAL_PARAMETER = 47;
const AL_DECODE_ERROR = 50;
const AL_PROTOCOL_VERSION = 70;

const AEAD_AES_GCM = 0;
const AEAD_CHACHA20 = 1;

/// The largest plaintext a record may carry, and the largest ciphertext.
const MAX_PLAINTEXT = 16384;
const MAX_CIPHERTEXT = 16640;

/// The ServerHello random that means "this is a HelloRetryRequest": SHA-256 of
/// "HelloRetryRequest". A retry uses the ServerHello message type, so this
/// value is the only thing that distinguishes them.
const HRR_RANDOM = []u8{
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11,
    0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91,
    0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e,
    0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
};

// ---------------------------------------------------------------------------
// Cipher suites
// ---------------------------------------------------------------------------

/// A cipher suite as a *value*, not an overload set.
///
/// The same argument `std/hash.Hash` makes: inside a body generic over the
/// algorithm there is no type to dispatch on, because the suite is chosen at
/// run time by the peer. So the four things the record layer needs to know are
/// four fields, and the hash is itself a value of exactly that kind.
pub const Suite = struct { id: i64, h: hash.Hash, key_len: i64, aead: i64 };

/// The suite a code point names, or null for one this library does not do.
pub fn suite_of(id: i64) ?Suite {
    if (id == TLS_AES_128_GCM_SHA256) {
        return Suite{ .id = id, .h = hash.sha256_hash(), .key_len = 16, .aead = AEAD_AES_GCM };
    }
    if (id == TLS_AES_256_GCM_SHA384) {
        return Suite{ .id = id, .h = hash.sha384_hash(), .key_len = 32, .aead = AEAD_AES_GCM };
    }
    if (id == TLS_CHACHA20_POLY1305_SHA256) {
        return Suite{ .id = id, .h = hash.sha256_hash(), .key_len = 32, .aead = AEAD_CHACHA20 };
    }
    return null;
}

/// The suites offered, most preferred first.
const SUITES = []i64{ 0x1301, 0x1303, 0x1302 };

// ---------------------------------------------------------------------------
// The key schedule
// ---------------------------------------------------------------------------
//
// RFC 8446 section 7.1. These are `pub` deliberately: RFC 8448 publishes every
// intermediate secret of a handshake, and a test that can ask for them one at
// a time says *which* derivation is wrong, where a test that can only run a
// whole handshake says only that one of eleven is.

/// `HKDF-Expand-Label(secret, label, context, length)`.
///
/// The label is written into the wire format with its "tls13 " prefix, which
/// is what keeps a key derived for one purpose from colliding with one derived
/// for another -- and what keeps TLS 1.3's schedule disjoint from anything
/// else that uses HKDF with the same secret.
pub fn hkdf_expand_label(h: hash.Hash, secret: []u8, label: str, context: []u8,
                         length: i64) []u8 {
    const b = bytes.buf(64);
    bytes.put_u16(b, length);
    const lm = bytes.open8(b);
    bytes.put_str(b, "tls13 ");
    bytes.put_str(b, label);
    bytes.close8(b, lm);
    const cm = bytes.open8(b);
    bytes.put_all(b, context);
    bytes.close8(b, cm);
    return hash.hkdf_expand(h, secret, bytes.taken(b), length);
}

/// `Derive-Secret(secret, label, messages)` -- the transcript hash is already
/// computed by the caller, because it is needed at several points and the
/// caller is what knows which.
pub fn derive_secret(h: hash.Hash, secret: []u8, label: str, transcript: []u8) []u8 {
    return hkdf_expand_label(h, secret, label, transcript, h.digest_size);
}

/// The all-zero secret an extract with nothing to extract from uses.
fn zeros(h: hash.Hash) []u8 { return bytes.new(h.digest_size); }

// ---------------------------------------------------------------------------
// The record layer
// ---------------------------------------------------------------------------

/// One direction's traffic keys.
///
/// The AES key is expanded once and kept, because this library computes the
/// S-box in the field rather than looking it up and so pays a real cost per
/// expansion. ChaCha20-Poly1305 takes a raw key per call and has nothing to
/// keep.
const Keys = struct {
    key: []u8,
    iv: []u8,
    seq: u64,
    aead: i64,
    gcm: ?cipher.AesGcm,
    /// The nonce, rebuilt per record rather than allocated per record.
    nonce: []u8,
};

fn keys_from(s: Suite, secret: []u8) Keys {
    const empty = bytes.new(0);
    const key = hkdf_expand_label(s.h, secret, "key", empty, s.key_len);
    const iv = hkdf_expand_label(s.h, secret, "iv", empty, 12);
    var g: ?cipher.AesGcm = null;
    if (s.aead == AEAD_AES_GCM) { g = cipher.aes_gcm_init(key); }
    return Keys{ .key = key, .iv = iv, .seq = 0, .aead = s.aead, .gcm = g,
                 .nonce = bytes.new(12) };
}

/// The per-record nonce: the static IV with the sequence number exclusive-ored
/// into its low eight bytes, big-endian.
fn next_nonce(k: Keys) []u8 {
    bytes.copy(k.nonce, 0, k.iv, 0, 12);
    var i = 0;
    while (i < 8) : (i += 1) {
        k.nonce[11 - i] ^= u8(k.seq >> u64(i * 8));
    }
    return k.nonce;
}

/// A whole encrypted record: header, ciphertext and tag.
///
/// The additional data is the record header itself, which is what binds the
/// declared length to the contents -- a truncation or an extension of a record
/// changes the length and so fails the tag.
fn seal_record(k: Keys, content_type: i64, plain: []u8, at: i64, n: i64) []u8 {
    const inner = bytes.new(n + 1);
    bytes.copy(inner, 0, plain, at, n);
    inner[n] = u8(content_type);
    const rec = bytes.new(5);
    rec[0] = u8(CT_APPLICATION_DATA);
    rec[1] = 3;
    rec[2] = 3;
    bytes.put_be16(rec, 3, n + 1 + 16);
    const nonce = next_nonce(k);
    var body = bytes.new(0);
    if (k.aead == AEAD_AES_GCM) {
        body = cipher.aes_gcm_seal(k.gcm.?, nonce, rec, inner);
    } else {
        body = cipher.chacha20_poly1305_seal(k.key, nonce, rec, inner);
    }
    k.seq += 1;
    return bytes.concat(rec, body);
}

/// The inner plaintext of a record, tag checked.
fn open_record(k: Keys, header: []u8, body: []u8) ![]u8 {
    const nonce = next_nonce(k);
    var plain = bytes.new(0);
    if (k.aead == AEAD_AES_GCM) {
        plain = cipher.aes_gcm_open(k.gcm.?, nonce, header, body)
            catch return error.BadRecordMac;
    } else {
        plain = cipher.chacha20_poly1305_open(k.key, nonce, header, body)
            catch return error.BadRecordMac;
    }
    k.seq += 1;
    return plain;
}

/// The real content type of an inner plaintext: the last byte that is not
/// padding.
///
/// A record that is all padding has no content type and is an error rather
/// than an empty record, which is what RFC 8446 section 5.4 requires -- and is
/// what stops a peer from sending records that cost work and say nothing.
fn strip_padding(inner: []u8) !i64 {
    var i = array.len(inner) - 1;
    while (i >= 0) {
        if (inner[i] != 0) { return i; }
        i -= 1;
    }
    return error.BadRecord;
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// What a client needs to know before it can start.
///
/// `pinned` is how stage four of this item tells a connection which key must
/// have signed the CertificateVerify, without a certificate parser existing
/// yet. A caller that has one leaves it null and sets `roots` instead.
pub const Config = struct {
    /// The name sent in SNI and checked against the certificate.
    host: str,
    /// A key that is trusted outright, or null.
    pinned: ?x509.SigKey,
    /// The trust anchors a chain must reach, or null.
    roots: ?list.List[x509.Cert],
};

/// A configuration that trusts nothing, and so completes no handshake.
///
/// Useful only as a starting point; `pinned_config` and `roots_config` are the
/// two that can actually finish.
pub fn client_config(host: str) Config {
    return Config{ .host = host, .pinned = null, .roots = null };
}

/// A configuration that checks the peer's chain against a set of anchors.
pub fn roots_config(host: str, roots: list.List[x509.Cert]) Config {
    return Config{ .host = host, .pinned = null, .roots = roots };
}

/// A configuration that trusts exactly one key, whatever certificate arrives.
///
/// Honest about what it is: this checks that the peer holds the private half
/// of a key the caller already knows, which is key pinning and not a chain.
pub fn pinned_config(host: str, key: x509.SigKey) Config {
    return Config{ .host = host, .pinned = key, .roots = null };
}

/// What a server needs.
///
/// Ed25519 only, because it is the one signature this library can *produce*.
/// RSA signing needs a constant-time exponentiation and the Chinese remainder
/// theorem, neither of which is written, and ECDSA signing needs a nonce whose
/// generation is the classic way to lose a private key.
pub const ServerConfig = struct {
    /// The certificate chain, leaf first, each an encoded certificate.
    chain: [][]u8,
    /// The 32-byte Ed25519 seed whose public key the leaf carries.
    key: []u8,
    /// A group to insist on with a HelloRetryRequest, or 0 to take whatever
    /// the client offered a share for.
    require_group: i64,
    /// A cipher suite to insist on, or 0 to take the first one offered that
    /// this library does.
    require_suite: i64,
};

pub fn server_config(chain: [][]u8, key: []u8) ServerConfig {
    return ServerConfig{ .chain = chain, .key = key,
                         .require_group = 0, .require_suite = 0 };
}

/// A server that will do exactly one suite and one group, refusing or retrying
/// rather than settling for anything else.
///
/// Exposed because a client and a server that always agree on the first thing
/// they both know are a pair whose other paths nothing has ever run.
pub fn server_requiring(chain: [][]u8, key: []u8, suite: i64, group: i64) ServerConfig {
    return ServerConfig{ .chain = chain, .key = key,
                         .require_group = group, .require_suite = suite };
}

// ---------------------------------------------------------------------------
// The connection
// ---------------------------------------------------------------------------

const ROLE_CLIENT = 0;
const ROLE_SERVER = 1;

const ST_START = 0;
const ST_WAIT_SH = 1;
const ST_WAIT_EE = 2;
const ST_WAIT_CERT = 3;
const ST_WAIT_CV = 4;
const ST_WAIT_FINISHED = 5;
const ST_WAIT_CH = 6;
const ST_WAIT_CLIENT_FINISHED = 7;
const ST_CONNECTED = 8;
const ST_CLOSED = 9;

/// One end of a connection: the whole state machine, and no socket.
pub const Conn = struct {
    role: i64,
    state: i64,
    cfg: Config,
    scfg: ?ServerConfig,

    /// The group we made a share on, and the two halves of it.
    group: i64,
    secret: []u8,
    share: []u8,
    random: []u8,
    session_id: []u8,

    /// Negotiated. Starts as the mandatory-to-implement suite so that the
    /// record layer always has a hash to ask about, and is overwritten the
    /// moment a ServerHello says which one it really is.
    suite: Suite,

    hs_secret: []u8,
    master: []u8,
    c_hs: []u8, s_hs: []u8,
    c_ap: []u8, s_ap: []u8,

    rk: ?Keys, wk: ?Keys,

    /// Every handshake message so far, for the transcript hash.
    tr: bytes.Buf,
    /// Bytes arrived and not yet framed into records.
    inb: bytes.Buf,
    /// Handshake bytes framed out of records and not yet a whole message.
    hsb: bytes.Buf,
    /// Records to send.
    out: bytes.Buf,
    /// Application data received.
    app: bytes.Buf,

    /// The key that must have signed the CertificateVerify.
    peer_key: ?x509.SigKey,
    /// The ClientHello as sent, kept because a HelloRetryRequest is answered
    /// with the same message and one extension changed.
    hello: []u8,
    retried: bool,
    /// The cookie a HelloRetryRequest asked to have echoed.
    cookie: []u8,
    /// True once a close_notify has arrived.
    peer_closed: bool,
    /// True once the server has asked for a client certificate. This client
    /// has none and answers with an empty list, which is what RFC 8446
    /// section 4.4.2 requires rather than silence.
    cert_requested: bool,
};

fn new_conn(role: i64, cfg: Config) Conn {
    const empty = bytes.new(0);
    return Conn{
        .role = role, .state = ST_START, .cfg = cfg, .scfg = null,
        .group = 0, .secret = empty, .share = empty,
        .random = empty, .session_id = empty,
        .suite = suite_of(TLS_AES_128_GCM_SHA256).?,
        .hs_secret = empty, .master = empty,
        .c_hs = empty, .s_hs = empty, .c_ap = empty, .s_ap = empty,
        .rk = null, .wk = null,
        .tr = bytes.buf(1024), .inb = bytes.buf(2048), .hsb = bytes.buf(1024),
        .out = bytes.buf(1024), .app = bytes.buf(1024),
        .peer_key = null, .hello = empty, .retried = false, .cookie = empty,
        .peer_closed = false, .cert_requested = false,
    };
}

/// The hash of every handshake message so far.
fn transcript(c: Conn) []u8 {
    const d = c.suite.h.digest;
    return d(bytes.taken(c.tr));
}

pub fn established(c: Conn) bool { return c.state == ST_CONNECTED; }

/// Whatever there is to send, and the buffer is emptied.
pub fn pending(c: Conn) []u8 {
    const out = bytes.taken(c.out);
    bytes.reset(c.out);
    return out;
}

/// Whatever application data has arrived, and the buffer is emptied.
pub fn app_data(c: Conn) []u8 {
    const out = bytes.taken(c.app);
    bytes.reset(c.app);
    return out;
}

pub fn peer_closed(c: Conn) bool { return c.peer_closed; }

// ---------------------------------------------------------------------------
// Making a key share
// ---------------------------------------------------------------------------

fn make_share(c: Conn, group: i64) !void {
    c.group = group;
    if (group == GROUP_X25519) {
        c.secret = try crypto.random(32);
        c.share = curve.x25519_base(c.secret);
        return;
    }
    if (group == GROUP_SECP256R1 or group == GROUP_SECP384R1) {
        const curve = nist_curve(group);
        // A scalar out of range is possible and is simply retried; the chance
        // is about one in 2^32 and a loop is the standard answer.
        var tries = 0;
        while (tries < 8) : (tries += 1) {
            const s = try crypto.random(curve.size);
            const pub_point = nistec.derive(curve, s) catch continue;
            c.secret = s;
            c.share = pub_point;
            return;
        }
        return error.HandshakeFailed;
    }
    return error.NoSharedGroup;
}

fn nist_curve(group: i64) nistec.Curve {
    if (group == GROUP_SECP384R1) { return nistec.p384(); }
    return nistec.p256();
}

fn shared_secret(c: Conn, peer: []u8) ![]u8 {
    if (c.group == GROUP_X25519) {
        if (array.len(peer) != 32) { return error.IllegalParameter; }
        return curve.x25519(c.secret, peer) catch return error.IllegalParameter;
    }
    if (c.group == GROUP_SECP256R1 or c.group == GROUP_SECP384R1) {
        return nistec.ecdh(nist_curve(c.group), c.secret, peer)
            catch return error.IllegalParameter;
    }
    return error.NoSharedGroup;
}

// ---------------------------------------------------------------------------
// Reading the wire
// ---------------------------------------------------------------------------
//
// A cursor with a bound, the shape `std/der` uses and for the same reason: a
// nested structure is read by making a cursor over its contents, so a length
// that lies cannot reach past the field that declared it. Every read is
// checked; there is no "the peer would not do that".

const Cursor = struct { b: []u8, at: i64, end: i64 };

fn cursor(b: []u8) Cursor {
    return Cursor{ .b = b, .at = 0, .end = array.len(b) };
}

fn left(c: Cursor) i64 { return c.end - c.at; }

fn done(c: Cursor) bool { return c.at >= c.end; }

fn get_u8(c: Cursor) !i64 {
    if (left(c) < 1) { return error.BadHandshake; }
    const v = i64(c.b[c.at]);
    c.at += 1;
    return v;
}

fn get_u16(c: Cursor) !i64 {
    if (left(c) < 2) { return error.BadHandshake; }
    const v = bytes.be16(c.b, c.at);
    c.at += 2;
    return v;
}

fn get_u24(c: Cursor) !i64 {
    if (left(c) < 3) { return error.BadHandshake; }
    const v = bytes.be24(c.b, c.at);
    c.at += 3;
    return v;
}

/// The next `n` bytes, copied out.
fn get_bytes(c: Cursor, n: i64) ![]u8 {
    if (n < 0 or left(c) < n) { return error.BadHandshake; }
    const out = bytes.slice(c.b, c.at, c.at + n);
    c.at += n;
    return out;
}

/// A cursor over the next `n` bytes, which are then skipped.
fn get_sub(c: Cursor, n: i64) !Cursor {
    if (n < 0 or left(c) < n) { return error.BadHandshake; }
    const out = Cursor{ .b = c.b, .at = c.at, .end = c.at + n };
    c.at += n;
    return out;
}

/// A vector with an 8-, 16- or 24-bit length in front of it.
fn get_vec8(c: Cursor) !Cursor { return get_sub(c, try get_u8(c)); }
fn get_vec16(c: Cursor) !Cursor { return get_sub(c, try get_u16(c)); }
fn get_vec24(c: Cursor) !Cursor { return get_sub(c, try get_u24(c)); }

// ---------------------------------------------------------------------------
// Writing the wire
// ---------------------------------------------------------------------------

/// Open an extension: its type, then a placeholder for its length.
fn ext_open(b: bytes.Buf, typ: i64) i64 {
    bytes.put_u16(b, typ);
    return bytes.open16(b);
}

/// The signature schemes offered, in preference order.
const SCHEMES = []i64{
    0x0807,   // ed25519
    0x0403,   // ecdsa_secp256r1_sha256
    0x0503,   // ecdsa_secp384r1_sha384
    0x0804,   // rsa_pss_rsae_sha256
    0x0805,   // rsa_pss_rsae_sha384
    0x0806,   // rsa_pss_rsae_sha512
    0x0401,   // rsa_pkcs1_sha256      -- certificates only
    0x0501,   // rsa_pkcs1_sha384
    0x0601,   // rsa_pkcs1_sha512
};

/// The groups offered, in preference order.
const GROUPS = []i64{ 0x001d, 0x0017, 0x0018 };

fn build_client_hello(c: Conn) []u8 {
    const b = bytes.buf(512);
    // legacy_version is frozen at 1.2; supported_versions is what decides.
    bytes.put_u16(b, 0x0303);
    bytes.put_all(b, c.random);
    const sm = bytes.open8(b);
    bytes.put_all(b, c.session_id);
    bytes.close8(b, sm);

    const cs = bytes.open16(b);
    var i = 0;
    while (i < array.len(SUITES)) : (i += 1) { bytes.put_u16(b, SUITES[i]); }
    bytes.close16(b, cs);

    // One compression method, and it is "none". TLS 1.3 removed compression;
    // the field survives so that a 1.2 parser can read this far.
    bytes.put_u8(b, 1);
    bytes.put_u8(b, 0);

    const em = bytes.open16(b);

    if (c.cfg.host != "") {
        const m = ext_open(b, EXT_SERVER_NAME);
        const lm = bytes.open16(b);
        bytes.put_u8(b, 0);
        const nm = bytes.open16(b);
        bytes.put_str(b, c.cfg.host);
        bytes.close16(b, nm);
        bytes.close16(b, lm);
        bytes.close16(b, m);
    }

    const gm = ext_open(b, EXT_SUPPORTED_GROUPS);
    const glm = bytes.open16(b);
    i = 0;
    while (i < array.len(GROUPS)) : (i += 1) { bytes.put_u16(b, GROUPS[i]); }
    bytes.close16(b, glm);
    bytes.close16(b, gm);

    const am = ext_open(b, EXT_SIGNATURE_ALGORITHMS);
    const alm = bytes.open16(b);
    i = 0;
    while (i < array.len(SCHEMES)) : (i += 1) { bytes.put_u16(b, SCHEMES[i]); }
    bytes.close16(b, alm);
    bytes.close16(b, am);

    const vm = ext_open(b, EXT_SUPPORTED_VERSIONS);
    const vlm = bytes.open8(b);
    bytes.put_u16(b, 0x0304);
    bytes.close8(b, vlm);
    bytes.close16(b, vm);

    if (array.len(c.cookie) != 0) {
        const km = ext_open(b, EXT_COOKIE);
        const clm = bytes.open16(b);
        bytes.put_all(b, c.cookie);
        bytes.close16(b, clm);
        bytes.close16(b, km);
    }

    const km2 = ext_open(b, EXT_KEY_SHARE);
    const slm = bytes.open16(b);
    bytes.put_u16(b, c.group);
    const shm = bytes.open16(b);
    bytes.put_all(b, c.share);
    bytes.close16(b, shm);
    bytes.close16(b, slm);
    bytes.close16(b, km2);

    bytes.close16(b, em);
    return bytes.taken(b);
}

// ---------------------------------------------------------------------------
// Records out
// ---------------------------------------------------------------------------

fn emit_record(c: Conn, ct: i64, data: []u8, at: i64, n: i64) void {
    var pos = at;
    var stop = at + n;
    var first = true;
    while (first or pos < stop) {
        first = false;
        var take = stop - pos;
        if (take > MAX_PLAINTEXT) { take = MAX_PLAINTEXT; }
        if (c.wk) |k| {
            bytes.put_all(c.out, seal_record(k, ct, data, pos, take));
        } else {
            bytes.put_u8(c.out, ct);
            bytes.put_u16(c.out, 0x0303);
            bytes.put_u16(c.out, take);
            bytes.put_bytes(c.out, data, pos, take);
        }
        pos += take;
    }
    return;
}

fn frame_handshake(typ: i64, body: []u8) []u8 {
    const b = bytes.buf(array.len(body) + 4);
    bytes.put_u8(b, typ);
    bytes.put_u24(b, array.len(body));
    bytes.put_all(b, body);
    return bytes.taken(b);
}

/// A handshake message: framed, added to the transcript, and sent.
fn emit_handshake(c: Conn, typ: i64, body: []u8) void {
    const msg = frame_handshake(typ, body);
    bytes.put_all(c.tr, msg);
    emit_record(c, CT_HANDSHAKE, msg, 0, array.len(msg));
    return;
}

/// A message sent after the handshake is over -- a KeyUpdate, or a ticket.
///
/// It is *not* added to the transcript. The transcript is what the Finished
/// messages and the CertificateVerify are computed over, and it stops at the
/// handshake's end; a post-handshake message that joined it would make two
/// peers disagree about every later derivation.
fn emit_post_handshake(c: Conn, typ: i64, body: []u8) void {
    const msg = frame_handshake(typ, body);
    emit_record(c, CT_HANDSHAKE, msg, 0, array.len(msg));
    return;
}

fn emit_alert(c: Conn, level: i64, desc: i64) void {
    const a = bytes.new(2);
    a[0] = u8(level);
    a[1] = u8(desc);
    emit_record(c, CT_ALERT, a, 0, 2);
    return;
}

/// A fatal alert, and the connection is over.
fn fail(c: Conn, desc: i64) void {
    if (c.state != ST_CLOSED) { emit_alert(c, 2, desc); }
    c.state = ST_CLOSED;
    return;
}

/// Send a close_notify. A caller that stops without one has told the peer
/// nothing, and a peer that accepts a stream ending without one cannot tell a
/// finished transfer from a truncated one.
pub fn close(c: Conn) void {
    if (c.state == ST_CLOSED) { return; }
    emit_alert(c, 1, AL_CLOSE_NOTIFY);
    c.state = ST_CLOSED;
    return;
}

/// Queue application data. Only meaningful once the handshake is done.
pub fn write_app(c: Conn, data: []u8, at: i64, n: i64) !void {
    if (c.state != ST_CONNECTED) { return error.NotConnected; }
    emit_record(c, CT_APPLICATION_DATA, data, at, n);
    return;
}

// ---------------------------------------------------------------------------
// The key schedule, driven
// ---------------------------------------------------------------------------

fn derive_handshake_keys(c: Conn, ecdhe: []u8) void {
    const h = c.suite.h;
    const empty = bytes.new(0);
    const early = hash.hkdf_extract(h, zeros(h), zeros(h));
    const derived = derive_secret(h, early, "derived", h.digest(empty));
    c.hs_secret = hash.hkdf_extract(h, derived, ecdhe);
    const th = transcript(c);
    c.c_hs = derive_secret(h, c.hs_secret, "c hs traffic", th);
    c.s_hs = derive_secret(h, c.hs_secret, "s hs traffic", th);
    const derived2 = derive_secret(h, c.hs_secret, "derived", h.digest(empty));
    c.master = hash.hkdf_extract(h, derived2, zeros(h));
    return;
}

fn derive_app_keys(c: Conn) void {
    const h = c.suite.h;
    const th = transcript(c);
    c.c_ap = derive_secret(h, c.master, "c ap traffic", th);
    c.s_ap = derive_secret(h, c.master, "s ap traffic", th);
    return;
}

/// `HMAC(HKDF-Expand-Label(base, "finished", "", L), transcript_hash)`.
pub fn finished_data(h: hash.Hash, base: []u8, transcript_hash: []u8) []u8 {
    const key = hkdf_expand_label(h, base, "finished", bytes.new(0), h.digest_size);
    return hash.hmac(h, key, transcript_hash);
}

/// The bytes a CertificateVerify signs: 64 spaces, a context string, a zero,
/// and the transcript hash.
///
/// The 64 spaces and the context are what stop a signature made for one
/// purpose being replayed as another -- including a signature made by a TLS
/// 1.2 server, whose signed content had no such prefix.
pub fn certificate_verify_content(context: str, transcript_hash: []u8) []u8 {
    const b = bytes.buf(160);
    var i = 0;
    while (i < 64) : (i += 1) { bytes.put_u8(b, 0x20); }
    bytes.put_str(b, context);
    bytes.put_u8(b, 0);
    bytes.put_all(b, transcript_hash);
    return bytes.taken(b);
}

// ---------------------------------------------------------------------------
// Records in
// ---------------------------------------------------------------------------

/// Take bytes that arrived. Whatever they complete is processed; whatever they
/// do not is kept.
pub fn feed(c: Conn, data: []u8, at: i64, n: i64) !void {
    // A connection that has sent or received a fatal alert is over, and
    // nothing arriving afterwards is worth parsing. Without this a peer could
    // keep a failed connection working the state machine.
    if (c.state == ST_CLOSED) { return error.NotConnected; }
    bytes.put_bytes(c.inb, data, at, n);
    try drain_records(c);
    return;
}

fn drain_records(c: Conn) !void {
    var more = true;
    while (more) {
        more = false;
        if (c.inb.used >= 5) {
            const ct = i64(c.inb.data[0]);
            const len = bytes.be16(c.inb.data, 3);
            // A record longer than the limit is refused before it is buffered,
            // so a peer cannot make this allocate by lying about a length.
            if (len > MAX_CIPHERTEXT) {
                fail(c, AL_RECORD_OVERFLOW);
                return error.RecordOverflow;
            }
            if (c.inb.used >= 5 + len) {
                const header = bytes.slice(c.inb.data, 0, 5);
                const body = bytes.slice(c.inb.data, 5, 5 + len);
                bytes.drop_front(c.inb, 5 + len);
                try handle_record(c, ct, header, body);
                more = true;
            }
        }
    }
    return;
}

fn handle_record(c: Conn, ct: i64, header: []u8, body: []u8) !void {
    if (ct == CT_CHANGE_CIPHER_SPEC) {
        // Compatibility mode: a single 0x01, before the handshake is over, and
        // never part of the transcript. RFC 8446 appendix D.4.
        if (c.state == ST_CONNECTED or c.state == ST_CLOSED) {
            fail(c, AL_UNEXPECTED_MESSAGE);
            return error.UnexpectedMessage;
        }
        if (array.len(body) != 1 or body[0] != 1) {
            fail(c, AL_ILLEGAL_PARAMETER);
            return error.UnexpectedMessage;
        }
        return;
    }

    var inner = body;
    var typ = ct;
    if (c.rk) |k| {
        // Once keys are in place every record is an application_data record on
        // the outside, whatever it says on the inside.
        if (ct != CT_APPLICATION_DATA) {
            fail(c, AL_UNEXPECTED_MESSAGE);
            return error.UnexpectedMessage;
        }
        const plain = open_record(k, header, body) catch {
            fail(c, AL_BAD_RECORD_MAC);
            return error.BadRecordMac;
        };
        const cut = strip_padding(plain) catch {
            fail(c, AL_UNEXPECTED_MESSAGE);
            return error.BadRecord;
        };
        typ = i64(plain[cut]);
        inner = bytes.slice(plain, 0, cut);
    }

    if (typ == CT_ALERT) {
        if (array.len(inner) != 2) {
            fail(c, AL_DECODE_ERROR);
            return error.BadRecord;
        }
        if (i64(inner[1]) == AL_CLOSE_NOTIFY) {
            c.peer_closed = true;
            return;
        }
        c.state = ST_CLOSED;
        return error.PeerAlert;
    }

    if (typ == CT_HANDSHAKE) {
        if (array.len(inner) == 0) {
            fail(c, AL_DECODE_ERROR);
            return error.BadHandshake;
        }
        bytes.put_all(c.hsb, inner);
        try drain_handshake(c);
        return;
    }

    if (typ == CT_APPLICATION_DATA) {
        if (c.state != ST_CONNECTED) {
            fail(c, AL_UNEXPECTED_MESSAGE);
            return error.UnexpectedMessage;
        }
        bytes.put_all(c.app, inner);
        return;
    }

    fail(c, AL_UNEXPECTED_MESSAGE);
    return error.UnexpectedMessage;
}

/// The largest handshake message this will reassemble.
///
/// The wire format allows 2^24 - 1, which is sixteen megabytes a peer can make
/// this hold before anything has been authenticated. A certificate chain that
/// does not fit in 64 KiB is a chain nothing should be talking to.
const MAX_HANDSHAKE = 65536;

fn drain_handshake(c: Conn) !void {
    var more = true;
    while (more) {
        more = false;
        if (c.hsb.used >= 4) {
            const typ = i64(c.hsb.data[0]);
            const len = bytes.be24(c.hsb.data, 1);
            if (len > MAX_HANDSHAKE) {
                fail(c, AL_DECODE_ERROR);
                return error.BadHandshake;
            }
            if (c.hsb.used >= 4 + len) {
                const msg = bytes.slice(c.hsb.data, 0, 4 + len);
                bytes.drop_front(c.hsb, 4 + len);
                if (c.role == ROLE_CLIENT) {
                    try client_message(c, typ, msg);
                } else {
                    try server_message(c, typ, msg);
                }
                more = true;
            }
        }
    }
    return;
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

fn client_message(c: Conn, typ: i64, msg: []u8) !void {
    const body = bytes.slice(msg, 4, array.len(msg));
    if (c.state == ST_WAIT_SH and typ == HS_SERVER_HELLO) {
        try client_server_hello(c, msg, body);
        return;
    }
    if (c.state == ST_WAIT_EE and typ == HS_ENCRYPTED_EXTENSIONS) {
        bytes.put_all(c.tr, msg);
        c.state = ST_WAIT_CERT;
        return;
    }
    if (c.state == ST_WAIT_CERT and typ == HS_CERTIFICATE_REQUEST) {
        bytes.put_all(c.tr, msg);
        c.cert_requested = true;
        return;
    }
    if (c.state == ST_WAIT_CERT and typ == HS_CERTIFICATE) {
        bytes.put_all(c.tr, msg);
        try client_certificate(c, body);
        c.state = ST_WAIT_CV;
        return;
    }
    if (c.state == ST_WAIT_CV and typ == HS_CERTIFICATE_VERIFY) {
        // The signature covers the transcript *through the Certificate*, so it
        // is checked before this message joins the transcript.
        try client_certificate_verify(c, body);
        bytes.put_all(c.tr, msg);
        c.state = ST_WAIT_FINISHED;
        return;
    }
    if (c.state == ST_WAIT_FINISHED and typ == HS_FINISHED) {
        try client_finished(c, msg, body);
        return;
    }
    // A ticket is a resumption offer, and this library does not resume. It is
    // read and dropped rather than refused, because a server sends one
    // unprompted and refusing would end a working connection.
    if (c.state == ST_CONNECTED and typ == HS_NEW_SESSION_TICKET) { return; }
    if (c.state == ST_CONNECTED and typ == HS_KEY_UPDATE) {
        try key_update(c, body);
        return;
    }
    fail(c, AL_UNEXPECTED_MESSAGE);
    return error.UnexpectedMessage;
}

fn client_server_hello(c: Conn, msg: []u8, body: []u8) !void {
    const cur = cursor(body);
    const legacy = try get_u16(cur);
    const random = try get_bytes(cur, 32);
    const sid_len = try get_u8(cur);
    const sid = try get_bytes(cur, sid_len);
    const suite_id = try get_u16(cur);
    const comp = try get_u8(cur);
    if (comp != 0) {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }
    const exts = try get_vec16(cur);
    if (!done(cur)) {
        fail(c, AL_DECODE_ERROR);
        return error.BadHandshake;
    }
    // The session id must come back exactly, which is what stops a response
    // meant for one connection being replayed into another.
    if (!bytes.equal(sid, c.session_id)) {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }

    var version = 0;
    var share_group = -1;
    var share = bytes.new(0);
    var cookie = bytes.new(0);
    while (!done(exts)) {
        const et = try get_u16(exts);
        const elen = try get_u16(exts);
        const ev = try get_sub(exts, elen);
        if (et == EXT_SUPPORTED_VERSIONS) {
            version = try get_u16(ev);
        } else if (et == EXT_KEY_SHARE) {
            share_group = try get_u16(ev);
            if (!done(ev)) {
                const kn = try get_u16(ev);
                share = try get_bytes(ev, kn);
            }
        } else if (et == EXT_COOKIE) {
            const cn = try get_u16(ev);
            cookie = try get_bytes(ev, cn);
        }
    }
    // `legacy_version` is 1.2 on the wire whatever is meant; the extension is
    // the only thing that says 1.3, and its absence is a 1.2 server.
    if (version != 0x0304) {
        fail(c, AL_PROTOCOL_VERSION);
        return error.UnsupportedVersion;
    }

    const s = suite_of(suite_id) orelse {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.NoSharedSuite;
    };

    if (bytes.equal(random, HRR_RANDOM)) {
        // A HelloRetryRequest wears the ServerHello message type; the random
        // is what tells them apart.
        if (c.retried) {
            fail(c, AL_UNEXPECTED_MESSAGE);
            return error.UnexpectedMessage;
        }
        c.retried = true;
        c.suite = s;
        if (share_group < 0 or share_group == c.group) {
            fail(c, AL_ILLEGAL_PARAMETER);
            return error.IllegalParameter;
        }
        // RFC 8446 section 4.4.1: the first ClientHello is replaced in the
        // transcript by a synthetic message holding its hash, so that a server
        // need keep no state between the two flights.
        const d = c.suite.h.digest;
        const first = d(bytes.taken(c.tr));
        bytes.reset(c.tr);
        bytes.put_u8(c.tr, HS_MESSAGE_HASH);
        bytes.put_u24(c.tr, c.suite.h.digest_size);
        bytes.put_all(c.tr, first);
        bytes.put_all(c.tr, msg);
        c.cookie = cookie;
        try make_share(c, share_group);
        c.hello = build_client_hello(c);
        emit_handshake(c, HS_CLIENT_HELLO, c.hello);
        return;
    }

    if (c.retried and s.id != c.suite.id) {
        // A retry may not change its mind about the suite.
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }
    c.suite = s;
    if (share_group != c.group) {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }
    bytes.put_all(c.tr, msg);
    const ecdhe = try shared_secret(c, share);
    derive_handshake_keys(c, ecdhe);
    c.rk = keys_from(c.suite, c.s_hs);
    c.state = ST_WAIT_EE;
    return;
}

fn client_certificate(c: Conn, body: []u8) !void {
    const cur = cursor(body);
    const ctx_len = try get_u8(cur);
    if (ctx_len != 0) {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }
    const certs = try get_vec24(cur);
    if (!done(cur)) {
        fail(c, AL_DECODE_ERROR);
        return error.BadHandshake;
    }
    if (done(certs)) {
        fail(c, AL_BAD_CERTIFICATE);
        return error.BadCertificate;
    }
    // Every entry is read, so that a malformed one behind a well-formed leaf
    // is still a malformed message.
    var first = bytes.new(0);
    var rest: list.List[[]u8] = list.new();
    var n = 0;
    while (!done(certs)) {
        const dn = try get_u24(certs);
        const cert = try get_bytes(certs, dn);
        const en = try get_u16(certs);
        const skip = try get_sub(certs, en);
        if (n == 0) { first = cert; } else { list.push(rest, cert); }
        n += 1;
    }

    if (c.cfg.pinned) |k| {
        // Key pinning: the chain is not consulted at all, and the connection
        // is exactly as trustworthy as the caller's belief about that key.
        c.peer_key = k;
        return;
    }
    if (c.cfg.roots) |roots| {
        // The clock is read here rather than kept in the config, so that a
        // long-lived program does not go on believing a certificate that
        // expired while it was running.
        c.peer_key = try x509.verify_chain(first, list.to_array(rest), roots,
                                           c.cfg.host, time.now());
        return;
    }
    fail(c, AL_BAD_CERTIFICATE);
    return error.BadCertificate;
}

fn client_certificate_verify(c: Conn, body: []u8) !void {
    const cur = cursor(body);
    const scheme = try get_u16(cur);
    const sig_len = try get_u16(cur);
    const sig = try get_bytes(cur, sig_len);
    if (!done(cur)) {
        fail(c, AL_DECODE_ERROR);
        return error.BadHandshake;
    }
    // RFC 8446 section 4.4.3: PKCS#1 v1.5 may sign a certificate and may not
    // sign a CertificateVerify. Accepting one here would let a signature made
    // by a 1.2 server be replayed as a 1.3 one.
    if (scheme == x509.RSA_PKCS1_SHA256 or scheme == x509.RSA_PKCS1_SHA384
        or scheme == x509.RSA_PKCS1_SHA512) {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }
    const key = c.peer_key orelse {
        fail(c, AL_BAD_CERTIFICATE);
        return error.BadCertificate;
    };
    const content = certificate_verify_content(
        "TLS 1.3, server CertificateVerify", transcript(c));
    if (!x509.verify_signature(key, scheme, content, sig)) {
        fail(c, AL_DECODE_ERROR);
        return error.BadSignature;
    }
    return;
}

fn client_finished(c: Conn, msg: []u8, body: []u8) !void {
    const h = c.suite.h;
    const expect = finished_data(h, c.s_hs, transcript(c));
    if (!bytes.equal(expect, body)) {
        fail(c, AL_DECODE_ERROR);
        return error.BadFinished;
    }
    bytes.put_all(c.tr, msg);
    // The application secrets are taken here, over the transcript that ends
    // with the server's Finished -- before this client adds anything of its
    // own to it.
    derive_app_keys(c);

    // The client's flight goes out under the *handshake* keys.
    c.wk = keys_from(c.suite, c.c_hs);
    if (c.cert_requested) {
        // No certificate: RFC 8446 section 4.4.2 spells that as an empty
        // certificate list rather than as silence.
        const b = bytes.buf(8);
        bytes.put_u8(b, 0);
        bytes.put_u24(b, 0);
        emit_handshake(c, HS_CERTIFICATE, bytes.taken(b));
    }
    const vd = finished_data(h, c.c_hs, transcript(c));
    emit_handshake(c, HS_FINISHED, vd);
    c.wk = keys_from(c.suite, c.c_ap);
    c.rk = keys_from(c.suite, c.s_ap);
    c.state = ST_CONNECTED;
    return;
}

// ---------------------------------------------------------------------------
// Key update
// ---------------------------------------------------------------------------

fn advance(h: hash.Hash, secret: []u8) []u8 {
    return hkdf_expand_label(h, secret, "traffic upd", bytes.new(0), h.digest_size);
}

/// Retire this end's write key and tell the peer.
///
/// `ask_peer` makes it a request that the peer do the same, which RFC 8446
/// section 4.6.3 allows exactly once per update -- an answer never asks back,
/// which is what stops two peers updating each other for ever.
///
/// The message itself goes out under the *old* key, because the peer has to be
/// able to read it.
pub fn request_key_update(c: Conn, ask_peer: bool) !void {
    if (c.state != ST_CONNECTED) { return error.NotConnected; }
    const b = bytes.new(1);
    if (ask_peer) { b[0] = 1; }
    emit_post_handshake(c, HS_KEY_UPDATE, b);
    const h = c.suite.h;
    if (c.role == ROLE_CLIENT) {
        c.c_ap = advance(h, c.c_ap);
        c.wk = keys_from(c.suite, c.c_ap);
    } else {
        c.s_ap = advance(h, c.s_ap);
        c.wk = keys_from(c.suite, c.s_ap);
    }
    return;
}

fn key_update(c: Conn, body: []u8) !void {
    const cur = cursor(body);
    const request = try get_u8(cur);
    if (!done(cur) or request > 1) {
        fail(c, AL_DECODE_ERROR);
        return error.BadHandshake;
    }
    const h = c.suite.h;
    if (c.role == ROLE_CLIENT) {
        c.s_ap = advance(h, c.s_ap);
        c.rk = keys_from(c.suite, c.s_ap);
    } else {
        c.c_ap = advance(h, c.c_ap);
        c.rk = keys_from(c.suite, c.c_ap);
    }
    if (request == 1) {
        // Answer with an update of our own, and never ask for one back --
        // which is what stops two peers updating each other for ever.
        emit_post_handshake(c, HS_KEY_UPDATE, bytes.new(1));
        if (c.role == ROLE_CLIENT) {
            c.c_ap = advance(h, c.c_ap);
            c.wk = keys_from(c.suite, c.c_ap);
        } else {
            c.s_ap = advance(h, c.s_ap);
            c.wk = keys_from(c.suite, c.s_ap);
        }
    }
    return;
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------
//
// Not a thing to deploy. It exists because a client that can only be tested
// against recorded bytes cannot be tested on the bytes it *produces*, and
// because `https://` is not proven by a transcript replay. It does one thing
// each way -- one certificate chain, one signature algorithm, no client
// authentication, no tickets -- and says so.

fn emit_ccs(c: Conn) void {
    // Always in the clear, never in the transcript: it exists only so that a
    // middlebox watching for a TLS 1.2 handshake sees the shape of one.
    bytes.put_u8(c.out, CT_CHANGE_CIPHER_SPEC);
    bytes.put_u16(c.out, 0x0303);
    bytes.put_u16(c.out, 1);
    bytes.put_u8(c.out, 1);
    return;
}

fn we_do_group(g: i64) bool {
    return g == GROUP_X25519 or g == GROUP_SECP256R1 or g == GROUP_SECP384R1;
}

fn server_message(c: Conn, typ: i64, msg: []u8) !void {
    const body = bytes.slice(msg, 4, array.len(msg));
    if (c.state == ST_WAIT_CH and typ == HS_CLIENT_HELLO) {
        try server_client_hello(c, msg, body);
        return;
    }
    if (c.state == ST_WAIT_CLIENT_FINISHED and typ == HS_FINISHED) {
        const expect = finished_data(c.suite.h, c.c_hs, transcript(c));
        if (!bytes.equal(expect, body)) {
            fail(c, AL_DECODE_ERROR);
            return error.BadFinished;
        }
        bytes.put_all(c.tr, msg);
        c.rk = keys_from(c.suite, c.c_ap);
        c.state = ST_CONNECTED;
        return;
    }
    if (c.state == ST_CONNECTED and typ == HS_KEY_UPDATE) {
        try key_update(c, body);
        return;
    }
    fail(c, AL_UNEXPECTED_MESSAGE);
    return error.UnexpectedMessage;
}

fn server_client_hello(c: Conn, msg: []u8, body: []u8) !void {
    const scfg = c.scfg orelse {
        fail(c, AL_HANDSHAKE_FAILURE);
        return error.HandshakeFailed;
    };
    const cur = cursor(body);
    const legacy = try get_u16(cur);
    const random = try get_bytes(cur, 32);
    const sid_len = try get_u8(cur);
    const sid = try get_bytes(cur, sid_len);
    const cs_len = try get_u16(cur);
    const offered = try get_bytes(cur, cs_len);
    const comp_len = try get_u8(cur);
    const comps = try get_bytes(cur, comp_len);
    const exts = try get_vec16(cur);
    if (!done(cur)) {
        fail(c, AL_DECODE_ERROR);
        return error.BadHandshake;
    }

    var have_13 = false;
    var ks_group = -1;
    var ks_key = bytes.new(0);
    var groups = bytes.new(0);
    var cookie = bytes.new(0);
    while (!done(exts)) {
        const et = try get_u16(exts);
        const elen = try get_u16(exts);
        const ev = try get_sub(exts, elen);
        if (et == EXT_SUPPORTED_VERSIONS) {
            const vl = try get_u8(ev);
            const vs = try get_bytes(ev, vl);
            var j = 0;
            const vn = array.len(vs);
            while (j + 1 < vn) : (j += 2) {
                if (bytes.be16(vs, j) == 0x0304) { have_13 = true; }
            }
        } else if (et == EXT_KEY_SHARE) {
            const kl = try get_u16(ev);
            const ks = try get_sub(ev, kl);
            while (!done(ks)) {
                const g = try get_u16(ks);
                const kn = try get_u16(ks);
                const kb = try get_bytes(ks, kn);
                if (ks_group < 0 and acceptable(scfg, g)) {
                    ks_group = g;
                    ks_key = kb;
                }
            }
        } else if (et == EXT_SUPPORTED_GROUPS) {
            const gl = try get_u16(ev);
            groups = try get_bytes(ev, gl);
        } else if (et == EXT_COOKIE) {
            const cl = try get_u16(ev);
            cookie = try get_bytes(ev, cl);
        }
    }
    if (!have_13) {
        fail(c, AL_PROTOCOL_VERSION);
        return error.UnsupportedVersion;
    }

    var chosen = 0;
    if (scfg.require_suite != 0) {
        if (offers(offered, scfg.require_suite)) { chosen = scfg.require_suite; }
    } else {
        var i = 0;
        const sn = array.len(SUITES);
        while (i < sn) : (i += 1) {
            if (chosen == 0 and offers(offered, SUITES[i])) { chosen = SUITES[i]; }
        }
    }
    if (chosen == 0) {
        fail(c, AL_HANDSHAKE_FAILURE);
        return error.NoSharedSuite;
    }
    c.suite = suite_of(chosen).?;

    if (ks_group < 0) {
        // Nothing usable was offered a share for. Ask again, naming a group
        // the client says it can do.
        if (c.retried) {
            fail(c, AL_HANDSHAKE_FAILURE);
            return error.NoSharedGroup;
        }
        const want = wanted_group(scfg, groups);
        if (want == 0) {
            fail(c, AL_HANDSHAKE_FAILURE);
            return error.NoSharedGroup;
        }
        // The first ClientHello becomes a synthetic message holding its hash,
        // RFC 8446 section 4.4.1.
        const d = c.suite.h.digest;
        const first = d(msg);
        bytes.reset(c.tr);
        bytes.put_u8(c.tr, HS_MESSAGE_HASH);
        bytes.put_u24(c.tr, c.suite.h.digest_size);
        bytes.put_all(c.tr, first);
        // A cookie, so the client's echo is exercised and so a second hello
        // that did not come from this exchange is refused.
        c.cookie = first;
        emit_handshake(c, HS_SERVER_HELLO, build_hello(c, sid, want, true));
        emit_ccs(c);
        c.retried = true;
        return;
    }
    if (c.retried and !bytes.equal(cookie, c.cookie)) {
        fail(c, AL_ILLEGAL_PARAMETER);
        return error.IllegalParameter;
    }

    bytes.put_all(c.tr, msg);
    try make_share(c, ks_group);
    c.random = try crypto.random(32);
    emit_handshake(c, HS_SERVER_HELLO, build_hello(c, sid, ks_group, false));
    emit_ccs(c);

    const ecdhe = try shared_secret(c, ks_key);
    derive_handshake_keys(c, ecdhe);
    c.wk = keys_from(c.suite, c.s_hs);
    c.rk = keys_from(c.suite, c.c_hs);
    try server_flight(c, scfg);
    return;
}

fn acceptable(scfg: ServerConfig, g: i64) bool {
    if (!we_do_group(g)) { return false; }
    if (scfg.require_group != 0) { return g == scfg.require_group; }
    return true;
}

/// A group to ask for in a HelloRetryRequest: the one demanded, if the client
/// says it can do it, or the first of ours that it lists.
fn wanted_group(scfg: ServerConfig, groups: []u8) i64 {
    var i = 0;
    const n = array.len(groups);
    while (i + 1 < n) : (i += 2) {
        const g = bytes.be16(groups, i);
        if (acceptable(scfg, g)) { return g; }
    }
    return 0;
}

fn offers(offered: []u8, id: i64) bool {
    var i = 0;
    const n = array.len(offered);
    while (i + 1 < n) : (i += 2) {
        if (bytes.be16(offered, i) == id) { return true; }
    }
    return false;
}

/// A ServerHello, or a HelloRetryRequest, which is the same message with a
/// fixed random and a group instead of a key.
fn build_hello(c: Conn, sid: []u8, group: i64, retry: bool) []u8 {
    const b = bytes.buf(160);
    bytes.put_u16(b, 0x0303);
    if (retry) {
        bytes.put_all(b, HRR_RANDOM);
    } else {
        bytes.put_all(b, c.random);
    }
    const sm = bytes.open8(b);
    bytes.put_all(b, sid);
    bytes.close8(b, sm);
    bytes.put_u16(b, c.suite.id);
    bytes.put_u8(b, 0);

    const em = bytes.open16(b);
    const km = ext_open(b, EXT_KEY_SHARE);
    bytes.put_u16(b, group);
    if (!retry) {
        const shm = bytes.open16(b);
        bytes.put_all(b, c.share);
        bytes.close16(b, shm);
    }
    bytes.close16(b, km);
    if (retry and array.len(c.cookie) != 0) {
        const cm = ext_open(b, EXT_COOKIE);
        const clm = bytes.open16(b);
        bytes.put_all(b, c.cookie);
        bytes.close16(b, clm);
        bytes.close16(b, cm);
    }
    const vm = ext_open(b, EXT_SUPPORTED_VERSIONS);
    bytes.put_u16(b, 0x0304);
    bytes.close16(b, vm);
    bytes.close16(b, em);
    return bytes.taken(b);
}

fn server_flight(c: Conn, scfg: ServerConfig) !void {
    // EncryptedExtensions, with nothing in it: this server answers no
    // extension that belongs here, and the message is not optional.
    const ee = bytes.buf(4);
    bytes.put_u16(ee, 0);
    emit_handshake(c, HS_ENCRYPTED_EXTENSIONS, bytes.taken(ee));

    const cb = bytes.buf(1024);
    bytes.put_u8(cb, 0);
    const lm = bytes.open24(cb);
    var i = 0;
    const n = array.len(scfg.chain);
    while (i < n) : (i += 1) {
        const dm = bytes.open24(cb);
        bytes.put_all(cb, scfg.chain[i]);
        bytes.close24(cb, dm);
        bytes.put_u16(cb, 0);
    }
    bytes.close24(cb, lm);
    emit_handshake(c, HS_CERTIFICATE, bytes.taken(cb));

    const content = certificate_verify_content(
        "TLS 1.3, server CertificateVerify", transcript(c));
    const sig = curve.ed25519_sign(scfg.key, content);
    const vb = bytes.buf(80);
    bytes.put_u16(vb, x509.ED25519);
    const sm = bytes.open16(vb);
    bytes.put_all(vb, sig);
    bytes.close16(vb, sm);
    emit_handshake(c, HS_CERTIFICATE_VERIFY, bytes.taken(vb));

    const vd = finished_data(c.suite.h, c.s_hs, transcript(c));
    emit_handshake(c, HS_FINISHED, vd);

    derive_app_keys(c);
    c.wk = keys_from(c.suite, c.s_ap);
    c.state = ST_WAIT_CLIENT_FINISHED;
    return;
}

// ---------------------------------------------------------------------------
// Starting one
// ---------------------------------------------------------------------------

/// A client that has written its ClientHello into `pending`.
pub fn client(cfg: Config) !Conn {
    const c = new_conn(ROLE_CLIENT, cfg);
    c.random = try crypto.random(32);
    // A 32-byte session id makes this look like a resumed 1.2 handshake to a
    // middlebox, which RFC 8446 appendix D.4 asks for and which is the whole
    // of "compatibility mode" on this side.
    c.session_id = try crypto.random(32);
    try make_share(c, GROUP_X25519);
    c.hello = build_client_hello(c);
    emit_handshake(c, HS_CLIENT_HELLO, c.hello);
    c.state = ST_WAIT_SH;
    return c;
}

/// A client that sends a ClientHello it was handed.
///
/// **A test hook**, in the style of `curve25519.field_mul`. RFC 8448 publishes
/// whole handshakes, and replaying one means sending the ClientHello it
/// recorded -- which this client would not otherwise produce, because that one
/// offers extensions this one does not. Everything downstream of the hello is
/// the real implementation, which is the point: what is being tested is the
/// key schedule, the record layer and the state machine against bytes nobody
/// here chose.
///
/// `hello` is the message body, without the four-byte handshake header.
pub fn client_replay(cfg: Config, hello: []u8, group: i64, secret: []u8) !Conn {
    const c = new_conn(ROLE_CLIENT, cfg);
    if (array.len(hello) < 35) { return error.BadHandshake; }
    c.random = bytes.slice(hello, 2, 34);
    const sid_len = i64(hello[34]);
    if (array.len(hello) < 35 + sid_len) { return error.BadHandshake; }
    c.session_id = bytes.slice(hello, 35, 35 + sid_len);
    c.group = group;
    c.secret = secret;
    c.hello = hello;
    emit_handshake(c, HS_CLIENT_HELLO, hello);
    c.state = ST_WAIT_SH;
    return c;
}

/// A server waiting for a ClientHello.
pub fn server(scfg: ServerConfig) Conn {
    const c = new_conn(ROLE_SERVER, Config{ .host = "", .pinned = null, .roots = null });
    c.scfg = scfg;
    c.state = ST_WAIT_CH;
    return c;
}

// ---------------------------------------------------------------------------
// Over a socket
// ---------------------------------------------------------------------------
//
// The thin part. Everything above works on buffers; this is what turns them
// into reads and writes, and it is deliberately the only place in the module
// that knows a socket exists.

pub const Session = struct { socket: net.Socket, conn: Conn, buf: []u8 };

fn flush(s: Session) !void {
    const out = pending(s.conn);
    const n = array.len(out);
    if (n > 0) { try net.write_all_bytes(s.socket, out, 0, n); }
    return;
}

fn pump(s: Session) !void {
    const n = try net.read_into(s.socket, s.buf, 0, array.len(s.buf));
    if (n == 0) { return error.EndOfFile; }
    try feed(s.conn, s.buf, 0, n);
    return;
}

fn run_handshake(s: Session) !void {
    var rounds = 0;
    while (!established(s.conn)) : (rounds += 1) {
        // A handshake is at most a few flights; a peer that keeps talking
        // without finishing one is not going to.
        if (rounds > 64) { return error.HandshakeFailed; }
        try flush(s);
        try pump(s);
        if (peer_closed(s.conn) and !established(s.conn)) {
            return error.HandshakeFailed;
        }
    }
    try flush(s);
    return;
}

/// A client handshake over a connected socket.
pub fn connect(socket: net.Socket, cfg: Config) !Session {
    const c = try client(cfg);
    const s = Session{ .socket = socket, .conn = c, .buf = bytes.new(MAX_CIPHERTEXT + 5) };
    try run_handshake(s);
    return s;
}

/// A server handshake over an accepted socket.
pub fn accept(socket: net.Socket, scfg: ServerConfig) !Session {
    const c = server(scfg);
    const s = Session{ .socket = socket, .conn = c, .buf = bytes.new(MAX_CIPHERTEXT + 5) };
    try run_handshake(s);
    return s;
}

/// Application data, or 0 at a clean end of stream.
pub fn read(s: Session, out: []u8, at: i64, max: i64) !i64 {
    while (s.conn.app.used == 0) {
        if (peer_closed(s.conn)) { return 0; }
        try pump(s);
        try flush(s);
    }
    var n = s.conn.app.used;
    if (n > max) { n = max; }
    bytes.copy(out, at, s.conn.app.data, 0, n);
    bytes.drop_front(s.conn.app, n);
    return n;
}

pub fn write_all(s: Session, data: []u8, at: i64, n: i64) !void {
    try write_app(s.conn, data, at, n);
    try flush(s);
    return;
}

/// Send a close_notify, then close the socket.
///
/// The alert is what tells the peer the stream ended rather than was cut, and
/// a failure to send it is not worth reporting: the socket is going anyway.
pub fn close_session(s: Session) void {
    close(s.conn);
    try_flush(s);
    net.close(s.socket);
    return;
}

fn try_flush(s: Session) void {
    flush(s) catch return;
    return;
}
