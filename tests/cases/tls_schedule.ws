// The TLS 1.3 key schedule, against RFC 8448's published traces.
//
// RFC 8448 is a whole handshake written down -- every message, and every
// secret derived from it. That makes it possible to check the schedule one
// derivation at a time rather than only end to end, which matters because a
// handshake has eleven of them and a whole-handshake test says only that one
// of the eleven is wrong.
//
// Twenty-three checks: every extract and every derive of section 3's 1-RTT
// exchange, both directions' traffic keys, the finished keys, the resumption
// secret, and section 5's handshake extract -- which is the same derivation
// over a P-256 shared secret instead of an X25519 one.
//
// The labels are the load-bearing part. `HKDF-Expand-Label` writes "tls13 "
// in front of every one, and that prefix is what keeps a key derived for one
// purpose from colliding with a key derived for another; a schedule that
// omitted it would produce self-consistent nonsense that no peer agrees with.
// expect: 1-rtt the early secret
// expect: 1-rtt derived for handshake
// expect: 1-rtt the handshake secret
// expect: 1-rtt c hs traffic
// expect: 1-rtt s hs traffic
// expect: 1-rtt derived for master
// expect: 1-rtt the master secret
// expect: 1-rtt write handshake key
// expect: 1-rtt write handshake iv
// expect: 1-rtt the finished key (1)
// expect: 1-rtt c ap traffic
// expect: 1-rtt s ap traffic
// expect: 1-rtt exp master
// expect: 1-rtt write application key (1)
// expect: 1-rtt write application iv (1)
// expect: 1-rtt read handshake key
// expect: 1-rtt read handshake iv
// expect: 1-rtt the finished key (2)
// expect: 1-rtt write application key (2)
// expect: 1-rtt write application iv (2)
// expect: 1-rtt res master
// expect: 1-rtt the resumption secret
// expect: retry the handshake secret
const tls = @import("std/tls");
const hash = @import("std/hash");
const bytes = @import("std/bytes");

fn main() i64 {
    extract("1-rtt the early secret", "", "0000000000000000000000000000000000000000000000000000000000000000", "33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a");
    derive("1-rtt derived for handshake", "33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a", "derived", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba");
    extract("1-rtt the handshake secret", "6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba", "8bd4054fb55b9d63fdfbacf9f04b9f0d35e6d63f537563efd46272900f89492d", "1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac");
    derive("1-rtt c hs traffic", "1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac", "c hs traffic", "860c06edc07858ee8e78f0e7428c58edd6b43f2ca3e6e95f02ed063cf0e1cad8", "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21");
    derive("1-rtt s hs traffic", "1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac", "s hs traffic", "860c06edc07858ee8e78f0e7428c58edd6b43f2ca3e6e95f02ed063cf0e1cad8", "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38");
    derive("1-rtt derived for master", "1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac", "derived", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "43de77e0c77713859a944db9db2590b53190a65b3ee2e4f12dd7a0bb7ce254b4");
    extract("1-rtt the master secret", "43de77e0c77713859a944db9db2590b53190a65b3ee2e4f12dd7a0bb7ce254b4", "0000000000000000000000000000000000000000000000000000000000000000", "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919");
    expand("1-rtt write handshake key", "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38", "key", 16, "3fce516009c21727d0f2e4e86ee403bc");
    expand("1-rtt write handshake iv", "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38", "iv", 12, "5d313eb2671276ee13000b30");
    expand("1-rtt the finished key (1)", "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38", "finished", 32, "008d3b66f816ea559f96b537e885c31fc068bf492c652f01f288a1d8cdc19fc8");
    derive("1-rtt c ap traffic", "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919", "c ap traffic", "9608102a0f1ccc6db6250b7b7e417b1a000eaada3daae4777a7686c9ff83df13", "9e40646ce79a7f9dc05af8889bce6552875afa0b06df0087f792ebb7c17504a5");
    derive("1-rtt s ap traffic", "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919", "s ap traffic", "9608102a0f1ccc6db6250b7b7e417b1a000eaada3daae4777a7686c9ff83df13", "a11af9f05531f856ad47116b45a950328204b4f44bfb6b3a4b4f1f3fcb631643");
    derive("1-rtt exp master", "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919", "exp master", "9608102a0f1ccc6db6250b7b7e417b1a000eaada3daae4777a7686c9ff83df13", "fe22f881176eda18eb8f44529e6792c50c9a3f89452f68d8ae311b4309d3cf50");
    expand("1-rtt write application key (1)", "a11af9f05531f856ad47116b45a950328204b4f44bfb6b3a4b4f1f3fcb631643", "key", 16, "9f02283b6c9c07efc26bb9f2ac92e356");
    expand("1-rtt write application iv (1)", "a11af9f05531f856ad47116b45a950328204b4f44bfb6b3a4b4f1f3fcb631643", "iv", 12, "cf782b88dd83549aadf1e984");
    expand("1-rtt read handshake key", "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21", "key", 16, "dbfaa693d1762c5b666af5d950258d01");
    expand("1-rtt read handshake iv", "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21", "iv", 12, "5bd3c71b836e0b76bb73265f");
    expand("1-rtt the finished key (2)", "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21", "finished", 32, "b80ad01015fb2f0bd65ff7d4da5d6bf83f84821d1f87fdc7d3c75b5a7b42d9c4");
    expand("1-rtt write application key (2)", "9e40646ce79a7f9dc05af8889bce6552875afa0b06df0087f792ebb7c17504a5", "key", 16, "17422dda596ed5d9acd890e3c63f5051");
    expand("1-rtt write application iv (2)", "9e40646ce79a7f9dc05af8889bce6552875afa0b06df0087f792ebb7c17504a5", "iv", 12, "5b78923dee08579033e523d9");
    derive("1-rtt res master", "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919", "res master", "209145a96ee8e2a122ff810047cc952684658d6049e86429426db87c54ad143d", "7df235f2031d2a051287d02b0241b0bfdaf86cc856231f2d5aba46c434ec196c");
    derive("1-rtt the resumption secret", "7df235f2031d2a051287d02b0241b0bfdaf86cc856231f2d5aba46c434ec196c", "resumption", "0000", "4ecd0eb6ec3b4d87f5d6028f922ca4c5851a277fd41311c9e62d2c9492e1c4f3");
    extract("retry the handshake secret", "6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba", "c142ce13ca11b5c2233652e63ad3d97844f1621fbfb9de69d547dc8fedeabeb4", "ce022e5e6e81e50736d773f2d3adfce8220d049bf510f0dbfac927ef4243b148");
    return 0;
}

/// `HKDF-Extract(salt, IKM)`. An empty salt stands for the all-zero one, which
/// is what the trace means by "0 (all zero octets)".
fn extract(name: str, salt: str, ikm: str, want: str) void {
    var s = hex(salt);
    if (salt == "") { s = bytes.new(32); }
    if (bytes.equal(hash.hkdf_extract(sha256(), s, hex(ikm)), hex(want))) {
        print(name);
    }
    return;
}

/// `Derive-Secret(secret, label, transcript_hash)`.
fn derive(name: str, prk: str, label: str, ctx: str, want: str) void {
    if (bytes.equal(tls.derive_secret(sha256(), hex(prk), label, hex(ctx)),
                    hex(want))) {
        print(name);
    }
    return;
}

/// `HKDF-Expand-Label(secret, label, "", length)`.
fn expand(name: str, prk: str, label: str, length: i64, want: str) void {
    if (bytes.equal(tls.hkdf_expand_label(sha256(), hex(prk), label,
                                          bytes.new(0), length),
                    hex(want))) {
        print(name);
    }
    return;
}

fn sha256() hash.Hash { return hash.sha256_hash(); }

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
