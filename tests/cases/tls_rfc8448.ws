// A whole TLS 1.3 handshake, replayed against RFC 8448 section 3.
//
// RFC 8448 publishes complete handshakes as bytes: every record, and every
// secret behind them. This case drives the client core with the server's
// recorded records and checks the records the client produces against the
// recorded ones -- byte for byte, which is the strongest statement a test can
// make about a protocol implementation.
//
// **The ClientHello is handed in rather than built**, through `client_replay`,
// and that is not a shortcut: the recorded client offers `renegotiation_info`,
// `session_ticket`, `record_size_limit` and a padding extension that this
// library does not, so a ClientHello built here would be a different message,
// the transcript hash would differ, and nothing downstream could be compared
// at all. Everything after the hello is this implementation: the key schedule,
// the record layer, the certificate handling, the signature check and the
// state machine. What is checked is what it *produces* -- the ClientHello is
// an input, so there would be no content in comparing it.
//
// The certificate is not parsed. Stage four of item 10 has no X.509 parser, so
// the key that must have signed the CertificateVerify is pinned, taken from
// the RFC's own section 2. Stage five replaces the pin with a chain and
// changes nothing else here.
//
// What each line proves, in order: the handshake completed at all, so every
// secret and both traffic keys agree with the server's; the client's Finished
// record is the recorded one, so the transcript hash and the finished key
// agree to the byte; the server's application data decrypts; the client's
// application record and its close_notify are the recorded ones, which pins
// the record nonce's sequence number as well as the keys.
// expect: the handshake completed
// expect: the client Finished record matches
// expect: 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031
// expect: the client application record matches
// expect: the peer said goodbye
// expect: the client close_notify matches
const tls = @import("std/tls");
const x509 = @import("std/x509");
const rsa = @import("std/rsa");
const bytes = @import("std/bytes");
const array = @import("std/array");

const PRIV = "49af42ba7f7994852d713ef2784bcbcaa7911de26adc5642cb634540e7ea5005";
const CH = "0303cb34ecb1e78163ba1c38c6dacb196a6dffa21a8d9912ec18a2ef6283024dece7000006130113031302010000910000000b0009000006736572766572ff01000100000a00140012001d0017001800190100010101020103010400230000003300260024001d002099381de560e4bd43d23d8e435a7dbafeb3c06e51c13cae4d5413691e529aaf2c002b0003020304000d0020001e040305030603020308040805080604010501060102010402050206020202002d00020101001c00024001";
const SH_REC = "160303005a020000560303a6af06a4121860dc5e6e60249cd34c95930c8ac5cb1434dac155772ed3e2692800130100002e00330024001d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f002b00020304";
const FLIGHT_REC = "17030302a2d1ff334a56f5bff6594a07cc87b580233f500f45e489e7f33af35edf7869fcf40aa40aa2b8ea73f848a7ca07612ef9f945cb960b4068905123ea78b111b429ba9191cd05d2a389280f526134aadc7fc78c4b729df828b5ecf7b13bd9aefb0e57f271585b8ea9bb355c7c79020716cfb9b1183ef3ab20e37d57a6b9d7477609aee6e122a4cf51427325250c7d0e509289444c9b3a648f1d71035d2ed65b0e3cdd0cbae8bf2d0b227812cbb360987255cc744110c453baa4fcd610928d809810e4b7ed1a8fd991f06aa6248204797e36a6a73b70a2559c09ead686945ba246ab66e5edd8044b4c6de3fcf2a89441ac66272fd8fb330ef8190579b3684596c960bd596eea520a56a8d650f563aad27409960dca63d3e688611ea5e22f4415cf9538d51a200c27034272968a264ed6540c84838d89f72c24461aad6d26f59ecaba9acbbb317b66d902f4f292a36ac1b639c637ce343117b659622245317b49eeda0c6258f100d7d961ffb138647e92ea330faeea6dfa31c7a84dc3bd7e1b7a6c7178af36879018e3f252107f243d243dc7339d5684c8b0378bf30244da8c87c843f5e56eb4c5e8280a2b48052cf93b16499a66db7cca71e4599426f7d461e66f99882bd89fc50800becca62d6c74116dbd2972fda1fa80f85df881edbe5a37668936b335583b599186dc5c6918a396fa48a181d6b6fa4f9d62d513afbb992f2b992f67f8afe67f76913fa388cb5630c8ca01e0c65d11c66a1e2ac4c85977b7c7a6999bbf10dc35ae69f5515614636c0b9b68c19ed2e31c0b3b66763038ebba42f3b38edc0399f3a9f23faa63978c317fc9fa66a73f60f0504de93b5b845e275592c12335ee340bbc4fddd502784016e4b3be7ef04dda49f4b440a30cb5d2af939828fd4ae3794e44f94df5a631ede42c1719bfdabf0253fe5175be898e750edc53370d2b";
const FIN_REC = "170303003575ec4dc238cce60b298044a71e219c56cc77b0517fe9b93c7a4bfc44d87f38f80338ac98fc46deb384bd1caeacab6867d726c40546";
const TICKET_REC = "17030300de3a6b8f90414a97d6959c3487680de5134a2b240e6cffac116e95d41d6af8f6b580dcf3d11d63c758db289a015940252f55713e061dc13e078891a38efbcf5753ad8ef170ad3c7353d16d9da773b9ca7f2b9fa1b6c0d4a3d03f75e09c30ba1e62972ac46f75f7b981be63439b2999ce13064615139891d5e4c5b406f16e3fc181a77ca475840025db2f0a77f81b5ab05b94c01346755f69232c86519d86cbeeac87aac347d143f9605d64f650db4d023e70e952ca49fe5137121c74bc2697687e248746d6df353005f3bce18696129c8153556b3b6c6779b37bf15985684f";
const CLIENT_APP_REC = "1703030043a23f7054b62c94d0affafe8228ba55cbefacea42f914aa66bcab3f2b9819a8a5b46b395bd54a9a20441e2b62974e1f5a6292a2977014bd1e3deae63aeebb21694915e4";
const SERVER_APP_REC = "17030300432e937e11ef4ac740e538ad36005fc4a46932fc3225d05f82aa1b36e30efaf97d90e6dffc602dcb501a59a8fcc49c4bf2e5f0a21c0047c2abf332540dd032e167c2955d";
const SERVER_ALERT_REC = "1703030013b58fd67166ebf599d24720cfbe7efa7a8864a9";
const CLIENT_ALERT_REC = "1703030013c9872760655666b74d7ff1153efd6db6d0b0e3";
const APP = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031";
const N = "b4bb498f8279303d980836399b36c6988c0c68de55e1bdb826d3901a2461eafd2de49a91d015abbc9a95137ace6c1af19eaa6af98c7ced43120998e187a80ee0ccb0524b1b018c3e0b63264d449a6d38e22a5fda430846748030530ef0461c8ca9d9efbfae8ea6d1d03e2bd193eff0ab9a8002c47428a6d35a8d88d79f7f1e3f";
const E = "010001";

fn main() i64 {
    // The server's certificate carries this key; RFC 8448 section 2 gives it
    // in the clear, which is what makes the signature checkable without a
    // certificate parser.
    const key: x509.SigKey = x509.RsaKey{
        .k = rsa.public_key(hex(N), hex(E)) catch return 1,
    };
    const c = tls.client_replay(tls.pinned_config("server", key), hex(CH),
                                tls.GROUP_X25519, hex(PRIV)) catch return 2;
    // The ClientHello record was handed in; drop it rather than compare it.
    const sent_hello = tls.pending(c);
    if (array.len(sent_hello) == 0) { return 3; }

    give(c, SH_REC) catch return 4;
    give(c, FLIGHT_REC) catch return 5;
    if (tls.established(c)) { print("the handshake completed"); }
    if (bytes.equal(tls.pending(c), hex(FIN_REC))) {
        print("the client Finished record matches");
    }

    // A ticket this library does not use, read and dropped -- and its record
    // still has to decrypt, because it is the first one under the server's
    // application keys and so pins that sequence number at zero.
    give(c, TICKET_REC) catch return 6;
    give(c, SERVER_APP_REC) catch return 7;
    print(bytes.to_hex(tls.app_data(c)));

    const app = hex(APP);
    tls.write_app(c, app, 0, array.len(app)) catch return 8;
    if (bytes.equal(tls.pending(c), hex(CLIENT_APP_REC))) {
        print("the client application record matches");
    }

    give(c, SERVER_ALERT_REC) catch return 9;
    if (tls.peer_closed(c)) { print("the peer said goodbye"); }
    tls.close(c);
    if (bytes.equal(tls.pending(c), hex(CLIENT_ALERT_REC))) {
        print("the client close_notify matches");
    }
    return 0;
}

fn give(c: tls.Conn, record: str) !void {
    const b = hex(record);
    try tls.feed(c, b, 0, array.len(b));
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
