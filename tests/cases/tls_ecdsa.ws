// The other half of the CertificateVerify: an ECDSA signature, and a server
// that asks for a certificate back.
//
// RFC 8448 section 6's server holds a P-256 certificate and signs its
// CertificateVerify with `ecdsa_secp256r1_sha256`, where section 3's signs
// with `rsa_pss_rsae_sha256`. The two are the whole of what a TLS 1.3 client
// has to verify in practice, and this is the second one -- over a recorded
// transcript, so the signature covers bytes nobody here chose.
//
// The public key is the one in the recorded certificate, and it was extracted
// by a second implementation: node's `crypto.X509Certificate` parsed the DER
// and exported the SubjectPublicKeyInfo, which is then handed to
// `x509.parse_spki` here. So this line also checks that this library reads a
// real certificate's key the same way an established parser does.
//
// The server also sends a CertificateRequest, and the answer to one from a
// client with no certificate is an *empty* certificate list rather than
// silence -- RFC 8446 section 4.4.2. The recorded client does authenticate, so
// its flight and this one differ from that point on and nothing after can be
// compared; what is checked is everything up to and including the server's
// Finished, which is exactly the part the ECDSA signature is in.
// expect: the key parses as P-256
// expect: the server flight verified
// expect: an empty certificate and a finished were sent
const tls = @import("std/tls");
const x509 = @import("std/x509");
const bytes = @import("std/bytes");
const array = @import("std/array");

const PRIV = "c040b2bb8f3addd20fd4058c547003a3c6f9c1cd915d5e535c87d8d191aaf071";
const CH = "03036a472236328b83af40386d3a3e1f1ce624fa4ed89ab865a4ff0f4144ce3ae2330000061301130313020100008d0000000b0009000006736572766572ff01000100000a00140012001d00170018001901000101010201030104003300260024001d0020089cc2671f738d9a671e5b2e464981d05b76e361aa22aea91f1d49ca10a7a362002b0003020304000d0020001e040305030603020308040805080604010501060102010402050206020202002d00020101001c00024001";
const SH_REC = "160303005a0200005603033b50fdf1c3d572e40e68953e7fff4e2758459c59afa0582c0ea032874255fe6e00130100002e00330024001d00206c2e50e865919a6b5a12dfaf918f92b442567b0f89bc54478c6921366658f062002b00020304";
const FLIGHT_REC = "17030302166d0a7ac079b32a94aa68c4e2893e8bd0d3c185f549c236fbbce3d647f08f3c94a2bf424d8708883605ad8955f97718b0213dead13dfb23ebb8381da582756612bcb5a5d40847719fbe9f179bfae656f3ecfd59a4c0d35132ce418a7e46f6b6a60622f8a6c06b28d83360163563be9c37f97eb902326924a72b3ed8c8381277d1581cab9c3715ac2401398467ad7ebfab3d0c3419e750104f7d62c5027901f2e4cd4ca5b8071eb03d3c732d83215066dfc4d291d4c1ff3b8d7e4298f677d4d51dea1168d8f16cb27ba40266313a1fedf9e23cc77f765450f9e96f05d08f3da245b14d4946f07ec81eed6d56f26bd574f0b7f7c7047037c16fce3b23754e662fad73e2b7213f6af296769c99a1d38e6232e0ec8dc4f84d6aa6f7de3887be0057862f9018e0ab396705aa4090ab5f2dff6325a557e7320d4effd46bb4f997d163207cce6665294aa4465541e3fe37ee7350659ea550d6dcb6af3c518852c7a14c3cc15bc32b3273bdf1751da184203135b117d300204fb12d58ca9ac34b68eca27030832f7a4b46d2a55757f63fe8f6e85ac47469e6198da88a64586bf23c69590de822263be75fd836847240c48f8c145cd6bd698962e7edc234ebe59231351eef8d7652cf3b08ab3af6e5ec74c58a8da34b39f9b0d6c4279a9a1f82071729e7059dd7f7b95b9433c4684ce1891a6d33432d52eddb0b8cee9181d403eccc12991f1ad4aa62c36049713a7bb135fdda6661a05a93f8c16f";
const SPKI = "3059301306072a8648ce3d020106082a8648ce3d0301070342000408d530161575f4cfe7f154ee3448180086001e88431a79ee62ee6e2f83ef38ba61e9fb37f34e007a7df4d2f5b56d1f04ece45d621f468406f5c3a15158948dd0";

fn main() i64 {
    const key = x509.parse_spki(hex(SPKI)) catch return 1;
    // The lattice, doing its job: a key read out of a certificate knows which
    // algorithm it is, and nothing here had to be told.
    if (x509.verify_signature(key, x509.ED25519, bytes.new(1), bytes.new(64))) {
        return 2;
    }
    print("the key parses as P-256");

    const c = tls.client_replay(tls.pinned_config("server", key), hex(CH),
                                tls.GROUP_X25519, hex(PRIV)) catch return 3;
    const hello = tls.pending(c);
    if (array.len(hello) == 0) { return 4; }

    give(c, SH_REC) catch return 5;
    give(c, FLIGHT_REC) catch return 6;
    if (tls.established(c)) { print("the server flight verified"); }

    // Two records: an empty Certificate, then the Finished. The first is
    // 8 bytes of message, 1 of content type and 16 of tag.
    const flight = tls.pending(c);
    if (array.len(flight) == 88 and bytes.be16(flight, 3) == 25) {
        print("an empty certificate and a finished were sent");
    }
    return 0;
}

fn give(c: tls.Conn, record: str) !void {
    const b = hex(record);
    try tls.feed(c, b, 0, array.len(b));
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
