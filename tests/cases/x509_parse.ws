// Reading a certificate, and deciding whether it is for this host.
//
// The certificates are minted for these tests rather than borrowed from
// anywhere: node cannot issue one, so the DER is written by hand in a script
// and signed with `crypto.sign`, and every certificate produced is handed back
// to node's own `crypto.X509Certificate` -- which is the independent check
// that what was written is what was meant. A borrowed certificate would expire
// and take the test with it.
//
// The wildcard lines are the ones worth reading twice. RFC 6125 section 6.4.3
// says a wildcard covers one label and only the leftmost one, and each of the
// four ways to get that wrong is a certificate for one name matching another:
// `*.a.com` matching `a.com` would make a certificate for a subdomain into one
// for the domain, and matching `c.b.a.com` would make it one for the whole
// tree.
// expect: the fields are read
// expect: not a certificate authority
// expect: two names
// expect: matches localhost
// expect: matches case-insensitively
// expect: a wildcard matches one label
// expect: a wildcard does not match the bare domain
// expect: a wildcard does not match two labels
// expect: a wildcard does not match another domain
// expect: an unrelated name does not match
// expect: the root is a certificate authority
// expect: the root signs certificates
// expect: the chain verifies
// expect: the same chain read from PEM verifies
// expect: a P-384 chain verifies
const x509 = @import("std/x509");
const bytes = @import("std/bytes");
const list = @import("std/list");
const text = @import("std/str");

const LEAF = "308201313081e4a00302010202011e300506032b657030233121301f06035504030c18777368617270207465737420696e7465726d656469617465301e170d3230303130313030303030305a170d3439313233313233353935395a301d311b301906035504030c12777368617270207465737420736572766572302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca3433041300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030210603551d11041a301882096c6f63616c686f7374820b2a2e77696c642e74657374300506032b65700341005b8b486c3336e4a16d5a24e3998189ee9277f8860e1a555d52f067ca2ec1082376b68b484969c062b76b217fad5b028b5f6517be84e3c0b4537745a8e8ac850c";
const INTER = "308201123081c5a003020102020114300506032b6570301b3119301706035504030c10777368617270207465737420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a30233121301f06035504030c18777368617270207465737420696e7465726d656469617465302a300506032b6570032100ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7ca326302430120603551d130101ff040830060101ff020100300e0603551d0f0101ff040403020106300506032b657003410052f9b98960d7f5552aa04136091af3eec6bd04368c24cbfc44e8e3c53faa12e46f6e1dd8046bffbd4854198dd3085ecd9aea1b64cb84cfc29005cb4f3c3f940f";
const ROOT = "308201073081baa00302010202010a300506032b6570301b3119301706035504030c10777368617270207465737420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c10777368617270207465737420726f6f74302a300506032b65700321008139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394a3233021300f0603551d130101ff040530030101ff300e0603551d0f0101ff040403020106300506032b65700341009b401f03050c341bde47090e5667f494535dbc0da7128718f37a76cd0974525c9a3adb7c736807315b10a642d7d625428ba7b1039db25154f1329959d6c6e10a";

/// A chain signed with ECDSA on P-384, which is the shape most of the world's
/// root certificates have -- and which nothing here could verify at all until
/// `std/nistec` grew the second curve.
const EC_ROOT = "308201843082010ba003020102020128300a06082a8648ce3d040303301b3119301706035504030c10777368617270207033383420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c10777368617270207033383420726f6f743076301006072a8648ce3d020106052b8104002203620004b014695c7b3708d632191eec41236ef6025528c724a4023e44fd228dea741ecb6f50bcc92297168ea3450df3560d8bfb698ca15e6f8d36c77d22bdf94a01074b2e93e44c117886fe881b5c69732c9c19a0a3ed7ad3e22c75150f262935f2aa81a3233021300f0603551d130101ff040530030101ff300e0603551d0f0101ff040403020106300a06082a8648ce3d040303036700306402304280a46d96fba17df85399a930107f6831a63cad1ed1780a712b25ed8fc38d4cdddd93f31ece133fb0ef7ade846aea46023067c646542246379758aa81e8b062fe68d13b5348f2538dcd996850d7299418cb88bc60d7e8030baf83d0127739625028";
const EC_LEAF = "3082019a30820120a003020102020129300a06082a8648ce3d040303301b3119301706035504030c10777368617270207033383420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a301d311b301906035504030c127773686172702070333834207365727665723076301006072a8648ce3d020106052b8104002203620004197a3ebb496e62f2cbf9d3bb860561fbc54307fbb234163a07206cc7284fc482052e57dbeed651429dd3c4688965dd832411de3367400adc19dd151d5feaeb74942a63641665e24ca3ec40d028214caf829f8f0e097388eb7cffa7d4aabee3c1a3363034300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030140603551d11040d300b82096c6f63616c686f7374300a06082a8648ce3d0403030368003065023100fad80392ad3adaada33af6bee577a231614e865c72beb4597c34f46028f8bbf85d0581398f756dbc2763113ece081527023007ef6799b68957499233748f384088eee322433fed38764ed6ee751850e3034460ed10f552acdc2aba23e6b981d4cb0c";

/// A fixed moment inside every fixture's validity, so that the case does not
/// depend on when it is run.
const NOW = 1700000000;

fn main() i64 {
    const leaf = x509.parse(hex(LEAF)) catch return 1;
    if (!bytes.equal(leaf.subject, leaf.issuer)
        and leaf.not_before < NOW and leaf.not_after > NOW) {
        print("the fields are read");
    }
    if (!leaf.is_ca and leaf.can_sign and leaf.for_server_auth) {
        print("not a certificate authority");
    }
    if (list.len(leaf.dns_names) == 2) { print("two names"); }

    if (x509.matches_host(leaf, "localhost")) { print("matches localhost"); }
    if (x509.matches_host(leaf, "LOCALHOST")) { print("matches case-insensitively"); }
    if (x509.matches_host(leaf, "a.wild.test")) { print("a wildcard matches one label"); }
    if (!x509.matches_host(leaf, "wild.test")) {
        print("a wildcard does not match the bare domain");
    }
    if (!x509.matches_host(leaf, "a.b.wild.test")) {
        print("a wildcard does not match two labels");
    }
    if (!x509.matches_host(leaf, "a.wild.test.evil.com")) {
        print("a wildcard does not match another domain");
    }
    if (!x509.matches_host(leaf, "example.com")) { print("an unrelated name does not match"); }

    const root = x509.parse(hex(ROOT)) catch return 2;
    if (root.is_ca) { print("the root is a certificate authority"); }
    if (root.can_sign_certs) { print("the root signs certificates"); }

    const roots = x509.parse_all([][]u8{ hex(ROOT) });
    x509.verify_chain(hex(LEAF), [][]u8{ hex(INTER) }, roots, "localhost", NOW)
        catch return 3;
    print("the chain verifies");

    // The same anchors, this time through the format a Unix machine keeps them
    // in -- which is the path `system_roots` takes on every Linux and BSD.
    const from_pem = x509.pem_certificates(pem(ROOT));
    if (list.len(from_pem) != 1) { return 4; }
    x509.verify_chain(hex(LEAF), [][]u8{ hex(INTER) }, from_pem, "localhost", NOW)
        catch return 5;
    print("the same chain read from PEM verifies");

    const ec_roots = x509.parse_all([][]u8{ hex(EC_ROOT) });
    x509.verify_chain(hex(EC_LEAF), [][]u8{}, ec_roots, "localhost", NOW)
        catch return 6;
    print("a P-384 chain verifies");
    return 0;
}

/// A certificate wrapped as PEM, so the reader has something to read.
fn pem(der: str) str {
    var out = "-----BEGIN CERTIFICATE-----\n";
    out = text.concat(out, bytes.to_base64(hex(der)));
    out = text.concat(out, "\n-----END CERTIFICATE-----\n");
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
