// A whole handshake between this library's two ends, with no sockets at all.
//
// RFC 8448 proves this client against bytes somebody else produced, which is
// the strongest thing a transcript can do and still leaves one thing untested:
// the bytes *this* library produces. So this case runs the client against the
// server through two in-memory buffers, over every combination that changes
// what the code does -- all three cipher suites, both key-agreement groups,
// and a HelloRetryRequest.
//
// No sockets, and that is not laziness. A blocking handshake writes its
// ClientHello and then waits for a reply, so one thread cannot be both ends of
// one; the existing socket cases get away with it because HTTP is
// request-then-response and TLS is a negotiation. Driving the two cores by
// hand is what makes the case single-threaded, deterministic, and free of any
// dependence on how a kernel schedules a loopback write. `tls_socket.ws` is
// where the blocking wrapper is proved instead.
//
// The three suites are not three ways of writing the same test. AES-128 and
// ChaCha20 differ in every byte of the record layer, and AES-256 is the one
// that runs the whole key schedule over SHA-384 -- a different digest length,
// a different HKDF block count and a different traffic key size.
//
// The certificate is a self-signed Ed25519 one, minted for these tests and
// handed back to node's `crypto.X509Certificate` to confirm it is a
// certificate. Stage four has no chain builder, so the client pins the key;
// what is being tested here is the handshake, not the trust decision.
// expect: aes-128-gcm over x25519 agreed
// expect: chacha20-poly1305 over x25519 agreed
// expect: aes-256-gcm over x25519 agreed
// expect: aes-128-gcm over p-256 agreed, after a retry
// expect: chacha20-poly1305 over p-256 agreed, after a retry
// expect: the client heard the server
// expect: the server heard the client
// expect: a record larger than one fragment survived
// expect: both ends still agree after a key update
// expect: the server said goodbye
const tls = @import("std/tls");
const x509 = @import("std/x509");
const bytes = @import("std/bytes");
const array = @import("std/array");

/// The seed the fixture certificate's key belongs to.
const SEED = "0101010101010101010101010101010101010101010101010101010101010101";
const CERT = "308201283081dba003020102020101300506032b6570301b3119301706035504030c107773686172702074657374206c656166301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c107773686172702074657374206c656166302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca3443042300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030220603551d11041b301982096c6f63616c686f7374820c6578616d706c652e74657374300506032b6570034100c5b3b0183715a28a33bbe5abd60e5e2a2679d235022b24b93699b05ab88d470ea8330b598b989ddf84609e9560c610e1c996685314ad8519f6562515baf31401";
const SPKI = "302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c";

fn main() i64 {
    const key = x509.parse_spki(hex(SPKI)) catch return 1;

    // Every combination that changes what the code does. The group is chosen
    // by the *server*, so asking for P-256 makes the client retry: it offered
    // a share for x25519 alone, which is what a first flight does.
    pair("aes-128-gcm over x25519 agreed", key, 0x1301, 0);
    pair("chacha20-poly1305 over x25519 agreed", key, 0x1303, 0);
    pair("aes-256-gcm over x25519 agreed", key, 0x1302, 0);
    pair("aes-128-gcm over p-256 agreed, after a retry", key, 0x1301, 0x0017);
    pair("chacha20-poly1305 over p-256 agreed, after a retry", key, 0x1303, 0x0017);

    // One connection, used.
    const cl = tls.client(tls.pinned_config("localhost", key)) catch return 2;
    const sv = tls.server(tls.server_config(chain(), hex(SEED)));
    handshake(cl, sv) catch return 3;

    if (says(sv, cl, "hello from the server")) { print("the client heard the server"); }
    if (says(cl, sv, "hello from the client")) { print("the server heard the client"); }

    // Larger than the 16384-byte record limit, so it has to be split and put
    // back together -- and the two records have different nonces.
    const big = bytes.new(20000);
    bytes.fill(big, 0, 20000, 0x5a);
    big[19999] = 0x99;
    tls.write_app(cl, big, 0, 20000) catch return 4;
    move(cl, sv) catch return 5;
    const got = tls.app_data(sv);
    if (array.len(got) == 20000 and got[0] == 0x5a and got[19999] == 0x99) {
        print("a record larger than one fragment survived");
    }

    // A key update retires the client's write key and asks the server to
    // retire its own, so both directions change.
    tls.request_key_update(cl, true) catch return 6;
    move(cl, sv) catch return 7;
    move(sv, cl) catch return 8;
    if (says(cl, sv, "after the update") and says(sv, cl, "and back")) {
        print("both ends still agree after a key update");
    }

    tls.close(sv);
    move(sv, cl) catch return 9;
    if (tls.peer_closed(cl)) { print("the server said goodbye"); }
    return 0;
}

/// A handshake with the server insisting on one suite and, when asked, one
/// group -- which is what makes the retry happen.
fn pair(note: str, key: x509.SigKey, suite: i64, group: i64) void {
    const scfg = tls.server_requiring(chain(), hex(SEED), suite, group);
    const cl = tls.client(tls.pinned_config("localhost", key)) catch return;
    const sv = tls.server(scfg);
    handshake(cl, sv) catch return;
    if (tls.established(cl) and tls.established(sv)) { print(note); }
    return;
}

fn chain() [][]u8 { return [][]u8{ hex(CERT) }; }

/// Drive both ends until they agree, or give up.
///
/// The bound is what turns a state machine that stops making progress into a
/// failing case rather than a hanging one.
fn handshake(cl: tls.Conn, sv: tls.Conn) !void {
    var rounds = 0;
    while (rounds < 16) : (rounds += 1) {
        const a = tls.pending(cl);
        if (array.len(a) > 0) { try tls.feed(sv, a, 0, array.len(a)); }
        const b = tls.pending(sv);
        if (array.len(b) > 0) { try tls.feed(cl, b, 0, array.len(b)); }
        if (tls.established(cl) and tls.established(sv)) { return; }
        if (array.len(a) == 0 and array.len(b) == 0) { return error.Stalled; }
    }
    return error.Stalled;
}

fn move(from: tls.Conn, to: tls.Conn) !void {
    const b = tls.pending(from);
    if (array.len(b) > 0) { try tls.feed(to, b, 0, array.len(b)); }
    return;
}

fn says(from: tls.Conn, to: tls.Conn, text: str) bool {
    const m = bytes.of(text);
    tls.write_app(from, m, 0, array.len(m)) catch return false;
    move(from, to) catch return false;
    return bytes.equal(tls.app_data(to), m);
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
