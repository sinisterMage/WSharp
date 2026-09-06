// Every way a TLS 1.3 exchange can go wrong that a peer can cause, a line
// each.
//
// The discipline is the AEADs' and the RSA verifier's, and it matters more
// here than anywhere else in item 10, because a TLS client's failures are all
// *acceptances*: a version it should have refused, a group it did not offer, a
// signature made by the wrong key, a record whose tag it did not really check.
// A single "a broken handshake fails" line would pass with any of those
// present.
//
// The crafted messages are built by hand rather than produced by this library,
// which is the point -- a server this library would never write is exactly the
// server a client meets. They are fed to a client started with RFC 8448's
// recorded ClientHello, because that one has an empty session id and so a
// hand-written ServerHello can echo it.
//
// Two refusals are *not* here and are worth naming. The signature checks
// themselves belong to `ed25519_reject.ws` and `p256_ecdsa_reject.ws`, which
// go through every way a signature can be wrong. And the ban on PKCS#1 v1.5 in
// a CertificateVerify (RFC 8446 section 4.4.3) is a test in front of the
// signature rather than a property of one, and reaching it needs a forged
// encrypted flight, which needs the handshake keys -- so it is checked by
// reading the code and not by this case.
// expect: a truncated record is waited for, not refused
// expect: a server that never says 1.3 refused
// expect: a server that says 1.2 refused
// expect: a suite that was not offered refused
// expect: a group that was not offered refused
// expect: a session id that is not ours refused
// expect: a second retry request refused
// expect: a fatal alert refused
// expect: a change cipher spec that is not one refused
// expect: application data before the handshake refused
// expect: a record over the size limit refused
// expect: a flipped ciphertext byte refused
// expect: a shortened record length refused
// expect: a certificate whose key did not sign refused
// expect: an empty certificate list refused
// expect: a flipped client Finished refused
// expect: a client that never says 1.3 refused
// expect: a client with no suite in common refused
// expect: a client with no group in common refused
const tls = @import("std/tls");
const x509 = @import("std/x509");
const rsa = @import("std/rsa");
const bytes = @import("std/bytes");
const array = @import("std/array");

const CH = "0303cb34ecb1e78163ba1c38c6dacb196a6dffa21a8d9912ec18a2ef6283024dece7000006130113031302010000910000000b0009000006736572766572ff01000100000a00140012001d0017001800190100010101020103010400230000003300260024001d002099381de560e4bd43d23d8e435a7dbafeb3c06e51c13cae4d5413691e529aaf2c002b0003020304000d0020001e040305030603020308040805080604010501060102010402050206020202002d00020101001c00024001";
const PRIV = "49af42ba7f7994852d713ef2784bcbcaa7911de26adc5642cb634540e7ea5005";
const SH_REC = "160303005a020000560303a6af06a4121860dc5e6e60249cd34c95930c8ac5cb1434dac155772ed3e2692800130100002e00330024001d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f002b00020304";
const FLIGHT_REC = "17030302a2d1ff334a56f5bff6594a07cc87b580233f500f45e489e7f33af35edf7869fcf40aa40aa2b8ea73f848a7ca07612ef9f945cb960b4068905123ea78b111b429ba9191cd05d2a389280f526134aadc7fc78c4b729df828b5ecf7b13bd9aefb0e57f271585b8ea9bb355c7c79020716cfb9b1183ef3ab20e37d57a6b9d7477609aee6e122a4cf51427325250c7d0e509289444c9b3a648f1d71035d2ed65b0e3cdd0cbae8bf2d0b227812cbb360987255cc744110c453baa4fcd610928d809810e4b7ed1a8fd991f06aa6248204797e36a6a73b70a2559c09ead686945ba246ab66e5edd8044b4c6de3fcf2a89441ac66272fd8fb330ef8190579b3684596c960bd596eea520a56a8d650f563aad27409960dca63d3e688611ea5e22f4415cf9538d51a200c27034272968a264ed6540c84838d89f72c24461aad6d26f59ecaba9acbbb317b66d902f4f292a36ac1b639c637ce343117b659622245317b49eeda0c6258f100d7d961ffb138647e92ea330faeea6dfa31c7a84dc3bd7e1b7a6c7178af36879018e3f252107f243d243dc7339d5684c8b0378bf30244da8c87c843f5e56eb4c5e8280a2b48052cf93b16499a66db7cca71e4599426f7d461e66f99882bd89fc50800becca62d6c74116dbd2972fda1fa80f85df881edbe5a37668936b335583b599186dc5c6918a396fa48a181d6b6fa4f9d62d513afbb992f2b992f67f8afe67f76913fa388cb5630c8ca01e0c65d11c66a1e2ac4c85977b7c7a6999bbf10dc35ae69f5515614636c0b9b68c19ed2e31c0b3b66763038ebba42f3b38edc0399f3a9f23faa63978c317fc9fa66a73f60f0504de93b5b845e275592c12335ee340bbc4fddd502784016e4b3be7ef04dda49f4b440a30cb5d2af939828fd4ae3794e44f94df5a631ede42c1719bfdabf0253fe5175be898e750edc53370d2b";
const SH_NO_VERSION = "1603030054020000500303aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00130100002800330024001d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f";
const SH_OLD_VERSION = "160303005a020000560303aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00130100002e00330024001d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f002b00020303";
const SH_BAD_SUITE = "160303005a020000560303aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00130500002e00330024001d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f002b00020304";
const SH_BAD_GROUP = "160303009b020000970303aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00130100006f0033006500180061bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb002b00020304";
const SH_BAD_SESSION = "160303007a020000760303aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa20cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc130100002e00330024001d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f002b00020304";
const HRR_REC = "1603030038020000340303cf21ad74e59a6111be1d8c021e65b891c2a211167abb8c5e079e09e2c8a8339c00130100000c003300020017002b00020304";
const ALERT_REC = "15030300020228";
const BAD_CCS_REC = "140303000100";
const EARLY_APP_REC = "1703030028dededededededededededededededededededededededededededededededededededededededede";
const HUGE_REC = "170303ffff0000000000000000";
const CH_NO_13 = "16030300610100005d03031111111111111111111111111111111111111111111111111111111111111111000002130101000032000a00040002001d003300260024001d00202222222222222222222222222222222222222222222222222222222222222222";
const CH_BAD_SUITE = "160303006a01000066030311111111111111111111111111111111111111111111111111111111111111110000041305c02f01000039002b0003020304000a00040002001d003300260024001d00202222222222222222222222222222222222222222222222222222222222222222";
const CH_BAD_GROUP = "16030300ab010000a70303111111111111111111111111111111111111111111111111111111111111111100000213010100007c002b0003020304000a00060004001800190033006700650018006133333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333";
const N = "b4bb498f8279303d980836399b36c6988c0c68de55e1bdb826d3901a2461eafd2de49a91d015abbc9a95137ace6c1af19eaa6af98c7ced43120998e187a80ee0ccb0524b1b018c3e0b63264d449a6d38e22a5fda430846748030530ef0461c8ca9d9efbfae8ea6d1d03e2bd193eff0ab9a8002c47428a6d35a8d88d79f7f1e3f";
const E = "010001";
const CERT = "308201283081dba003020102020101300506032b6570301b3119301706035504030c107773686172702074657374206c656166301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c107773686172702074657374206c656166302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca3443042300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030220603551d11041b301982096c6f63616c686f7374820c6578616d706c652e74657374300506032b6570034100c5b3b0183715a28a33bbe5abd60e5e2a2679d235022b24b93699b05ab88d470ea8330b598b989ddf84609e9560c610e1c996685314ad8519f6562515baf31401";
const SPKI = "302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c";

/// The fixture certificate's key, and a seed that is *not* the one it belongs
/// to -- which is what makes "the certificate says one key and the signature
/// was made by another" a case rather than a hypothetical.
const SEED = "0101010101010101010101010101010101010101010101010101010101010101";
const SEED_OTHER = "0303030303030303030303030303030303030303030303030303030303030303";

fn main() i64 {
    // The control: a record that has not all arrived is kept, not refused.
    // Without this line every refusal below passes with a client that gives up
    // on everything.
    const c0 = start() catch return 1;
    const half = bytes.slice(hex(SH_REC), 0, 40);
    tls.feed(c0, half, 0, array.len(half)) catch return 2;
    print("a truncated record is waited for, not refused");

    refuse(SH_NO_VERSION, "a server that never says 1.3 refused");
    refuse(SH_OLD_VERSION, "a server that says 1.2 refused");
    refuse(SH_BAD_SUITE, "a suite that was not offered refused");
    refuse(SH_BAD_GROUP, "a group that was not offered refused");
    refuse(SH_BAD_SESSION, "a session id that is not ours refused");
    refuse_twice(HRR_REC, "a second retry request refused");
    refuse(ALERT_REC, "a fatal alert refused");
    refuse(BAD_CCS_REC, "a change cipher spec that is not one refused");
    refuse(EARLY_APP_REC, "application data before the handshake refused");
    refuse(HUGE_REC, "a record over the size limit refused");

    // The record layer, from a real flight: one bit of ciphertext, and one bit
    // of the length that is also the additional data.
    refuse_flight(20, 1, "a flipped ciphertext byte refused");
    refuse_flight(4, 2, "a shortened record length refused");

    // The trust decision, from a real server that is wrong in one way each.
    wrong_key();
    empty_chain();
    flipped_finished();

    // And the server's own refusals.
    refuse_server(CH_NO_13, "a client that never says 1.3 refused");
    refuse_server(CH_BAD_SUITE, "a client with no suite in common refused");
    refuse_server(CH_BAD_GROUP, "a client with no group in common refused");
    return 0;
}

/// A client that has sent RFC 8448's ClientHello and is waiting for an answer.
fn start() !tls.Conn {
    const key: x509.SigKey = x509.RsaKey{ .k = try rsa.public_key(hex(N), hex(E)) };
    const c = try tls.client_replay(tls.pinned_config("server", key), hex(CH),
                                    tls.GROUP_X25519, hex(PRIV));
    const sent = tls.pending(c);
    if (array.len(sent) == 0) { return error.Stalled; }
    return c;
}

fn refuse(record: str, note: str) void {
    const c = start() catch return;
    give(c, record) catch { print(note); return; };
    return;
}

/// The same record twice, for the retry that may happen only once.
fn refuse_twice(record: str, note: str) void {
    const c = start() catch return;
    give(c, record) catch return;
    give(c, record) catch { print(note); return; };
    return;
}

/// A real server flight with one bit changed.
fn refuse_flight(at: i64, mask: u8, note: str) void {
    const c = start() catch return;
    give(c, SH_REC) catch return;
    const f = hex(FLIGHT_REC);
    f[at] ^= mask;
    tls.feed(c, f, 0, array.len(f)) catch { print(note); return; };
    return;
}

fn chain() [][]u8 { return [][]u8{ hex(CERT) }; }

/// The certificate carries one key and the CertificateVerify was made with
/// another. This is the check that a pinned or chained key is actually *used*
/// rather than merely stored.
fn wrong_key() void {
    const key = x509.parse_spki(hex(SPKI)) catch return;
    const cl = tls.client(tls.pinned_config("localhost", key)) catch return;
    const sv = tls.server(tls.server_config(chain(), hex(SEED_OTHER)));
    if (!exchange(cl, sv)) { print("a certificate whose key did not sign refused"); }
    return;
}

fn empty_chain() void {
    const key = x509.parse_spki(hex(SPKI)) catch return;
    const cl = tls.client(tls.pinned_config("localhost", key)) catch return;
    const sv = tls.server(tls.server_config([][]u8{}, hex(SEED)));
    if (!exchange(cl, sv)) { print("an empty certificate list refused"); }
    return;
}

/// A real handshake, with one bit of the client's Finished changed on the way.
fn flipped_finished() void {
    const key = x509.parse_spki(hex(SPKI)) catch return;
    const cl = tls.client(tls.pinned_config("localhost", key)) catch return;
    const sv = tls.server(tls.server_config(chain(), hex(SEED)));
    move(cl, sv) catch return;
    move(sv, cl) catch return;
    const fin = tls.pending(cl);
    if (array.len(fin) == 0) { return; }
    fin[array.len(fin) - 1] ^= 1;
    tls.feed(sv, fin, 0, array.len(fin)) catch {
        print("a flipped client Finished refused");
        return;
    };
    return;
}

fn refuse_server(record: str, note: str) void {
    const sv = tls.server(tls.server_config(chain(), hex(SEED)));
    give(sv, record) catch { print(note); return; };
    return;
}

/// True when both ends completed. Used only for its failures.
fn exchange(cl: tls.Conn, sv: tls.Conn) bool {
    var rounds = 0;
    while (rounds < 8) : (rounds += 1) {
        move(cl, sv) catch return false;
        move(sv, cl) catch return false;
        if (tls.established(cl) and tls.established(sv)) { return true; }
    }
    return false;
}

fn move(from: tls.Conn, to: tls.Conn) !void {
    const b = tls.pending(from);
    if (array.len(b) > 0) { try tls.feed(to, b, 0, array.len(b)); }
    return;
}

fn give(c: tls.Conn, record: str) !void {
    const b = hex(record);
    try tls.feed(c, b, 0, array.len(b));
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
