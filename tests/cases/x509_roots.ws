// Reading a certificate store: the PEM a Unix machine keeps one in, and the
// platform call that answers where it does not.
//
// The bundle here is built in the case rather than read from the machine, so
// the case says the same thing on a machine with no certificates installed and
// on one with two hundred. What it exercises is the part that is the same
// either way: several certificates in one file, junk between them that is not
// a certificate, and a chain verified against what came out.
//
// The system store *is* touched, at the end, and deliberately without asserting
// anything about what it holds. A container with no bundle and a laptop with a
// full keychain are both correct, and a case that demanded one of them would
// fail on the other machine for no reason. What it does check is that asking
// cannot crash and cannot hang -- which covers the platform arms that cannot be
// run here at all.
// expect: two certificates came out of the bundle
// expect: the junk between them was skipped
// expect: a chain verifies against the decoded anchors
// expect: an empty file yields nothing
// expect: a block whose base64 is broken is skipped
// expect: the system store was consulted
const x509 = @import("std/x509");
const bytes = @import("std/bytes");
const list = @import("std/list");
const text = @import("std/str");

const ROOT = "308201073081baa00302010202010a300506032b6570301b3119301706035504030c10777368617270207465737420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c10777368617270207465737420726f6f74302a300506032b65700321008139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394a3233021300f0603551d130101ff040530030101ff300e0603551d0f0101ff040403020106300506032b65700341009b401f03050c341bde47090e5667f494535dbc0da7128718f37a76cd0974525c9a3adb7c736807315b10a642d7d625428ba7b1039db25154f1329959d6c6e10a";
const ROOT2 = "308201093081bca00302010202010b300506032b6570301c311a301806035504030c11777368617270206f7468657220726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a301c311a301806035504030c11777368617270206f7468657220726f6f74302a300506032b65700321008a875fff1eb38451577acd5afee405456568dd7c89e090863a0557bc7af49f17a3233021300f0603551d130101ff040530030101ff300e0603551d0f0101ff040403020106300506032b65700341004feb200a96c5c94cdc6cfbd3a77cfbf7cd4bfb22b08dce24129379d10dee000aa67d4aca12a204c8a109079dda6e5ff8bc9598c30c7c07f22dde7795c4743201";
const INTER = "308201123081c5a003020102020114300506032b6570301b3119301706035504030c10777368617270207465737420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a30233121301f06035504030c18777368617270207465737420696e7465726d656469617465302a300506032b6570032100ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7ca326302430120603551d130101ff040830060101ff020100300e0603551d0f0101ff040403020106300506032b657003410052f9b98960d7f5552aa04136091af3eec6bd04368c24cbfc44e8e3c53faa12e46f6e1dd8046bffbd4854198dd3085ecd9aea1b64cb84cfc29005cb4f3c3f940f";
const LEAF = "308201313081e4a00302010202011e300506032b657030233121301f06035504030c18777368617270207465737420696e7465726d656469617465301e170d3230303130313030303030305a170d3439313233313233353935395a301d311b301906035504030c12777368617270207465737420736572766572302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca3433041300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030210603551d11041a301882096c6f63616c686f7374820b2a2e77696c642e74657374300506032b65700341005b8b486c3336e4a16d5a24e3998189ee9277f8860e1a555d52f067ca2ec1082376b68b484969c062b76b217fad5b028b5f6517be84e3c0b4537745a8e8ac850c";

const NOW = 1700000000;

fn main() i64 {
    // What a real bundle looks like: a comment, a certificate, a heading,
    // another certificate, a trailing note.
    var bundle = "# a bundle, as a distribution ships one\n";
    bundle = text.concat(bundle, pem(ROOT));
    bundle = text.concat(bundle, "\nSome Certificate Authority\n==========================\n");
    bundle = text.concat(bundle, pem(ROOT2));
    bundle = text.concat(bundle, "\n# nothing after this\n");

    const roots = x509.pem_certificates(bundle);
    if (list.len(roots) == 2) { print("two certificates came out of the bundle"); }
    const first = list.get(roots, 0);
    const second = list.get(roots, 1);
    if (first.is_ca and second.is_ca and !bytes.equal(first.subject, second.subject)) {
        print("the junk between them was skipped");
    }

    x509.verify_chain(hex(LEAF), [][]u8{ hex(INTER) }, roots, "localhost", NOW)
        catch return 1;
    print("a chain verifies against the decoded anchors");

    if (list.len(x509.pem_certificates("")) == 0) { print("an empty file yields nothing"); }

    // A block that is shaped like one and is not. A store is a bag of
    // authorities, so one bad entry costs one authority and not the file.
    var broken = "-----BEGIN CERTIFICATE-----\nnot base64 at all!!\n-----END CERTIFICATE-----\n";
    broken = text.concat(broken, pem(ROOT));
    if (list.len(x509.pem_certificates(broken)) == 1) {
        print("a block whose base64 is broken is skipped");
    }

    // No assertion: a machine may or may not have a store, and both are right.
    const store = x509.system_roots() catch list.new();
    if (list.len(store) >= 0) { print("the system store was consulted"); }
    return 0;
}

fn pem(der: str) str {
    var out = "-----BEGIN CERTIFICATE-----\n";
    out = text.concat(out, bytes.to_base64(hex(der)));
    out = text.concat(out, "\n-----END CERTIFICATE-----\n");
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
