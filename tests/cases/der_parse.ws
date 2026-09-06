// Reading DER: the shapes a certificate is made of.
//
// Nothing here is a published vector, because DER is a format rather than an
// algorithm -- so the encodings below were built by hand from X.690 and each
// is the shape `std/x509` will meet: a nested SEQUENCE, an OID compared as
// bytes, a BIT STRING whose unused-bit count is zero, an explicitly tagged
// `[0]`, a long-form length, and both spellings of a time.
//
// The two time lines are the interesting pair. RFC 5280 section 4.1.2.5.1
// says a two-digit year below 50 is in the twenty-first century and one at 50
// or above is in the twentieth, so 49 and 50 are a hundred years apart and one
// off-by-one in that rule expires every certificate on the internet or none of
// them. Both epochs were computed independently before they were written here.
// expect: 1
// expect: 06082a8648ce3d030107
// expect: the curve is P-256
// expect: deadbeef
// expect: 2
// expect: nothing left over
// expect: 128
// expect: 1788696000
// expect: 1788696000
// expect: 2493072000
// expect: -631152000
// expect: 48
// expect: 301d020101300c06082a8648ce3d0301070500030500deadbeefa003020102
const der = @import("std/der");
const bytes = @import("std/bytes");
const array = @import("std/array");

/// `SEQUENCE { INTEGER 1, SEQUENCE { OID prime256v1, NULL }, BIT STRING,
/// [0] { INTEGER 2 } }` -- the shape of an AlgorithmIdentifier beside a key.
const GOOD = "301d020101300c06082a8648ce3d0301070500030500deadbeefa003020102";
/// 1.2.840.10045.3.1.7, which is what says a key is on P-256.
const OID_P256 = "06082a8648ce3d030107";
/// An OCTET STRING of 128 zero bytes: the shortest contents that must use the
/// long form of the length, and so the boundary the minimality rule is about.
const LONG = "0481800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

const UTC = "170d3236303930363132303030305a";
const GEN = "180f32303236303930363132303030305a";
const UTC_49 = "170d3439303130313030303030305a";
const UTC_50 = "170d3530303130313030303030305a";

fn main() i64 {
    const whole = hex(GOOD);
    const outer = der.reader(whole);
    const seq = der.read_seq(outer) catch return 1;
    der.expect_end(outer) catch return 2;

    print_int(der.read_uint(seq) catch return 3);

    // An algorithm identifier: the OID and its absent parameters.
    const alg = der.read_seq(seq) catch return 4;
    const oid = der.read_oid(alg) catch return 5;
    print(bytes.to_hex(prefixed(oid)));
    if (bytes.equal(prefixed(oid), hex(OID_P256))) { print("the curve is P-256"); }
    der.read_null(alg) catch return 6;
    der.expect_end(alg) catch return 7;

    print(bytes.to_hex(der.read_bitstring(seq) catch return 8));

    // `[0] EXPLICIT`, which is how a version is carried.
    if (der.has_tag(seq, der.context(0))) {
        const tagged = der.read_tagged(seq, der.context(0)) catch return 9;
        print_int(der.read_uint(tagged) catch return 10);
        der.expect_end(tagged) catch return 11;
    }
    if (der.at_end(seq)) { print("nothing left over"); }

    print_int(array.len(der.read_octets(der.reader(hex(LONG))) catch return 12));

    print_int(der.read_time(der.reader(hex(UTC))) catch return 13);
    print_int(der.read_time(der.reader(hex(GEN))) catch return 14);
    print_int(der.read_time(der.reader(hex(UTC_49))) catch return 15);
    print_int(der.read_time(der.reader(hex(UTC_50))) catch return 16);

    // A value can be located without being consumed, and read back whole --
    // which is what signing over a TBSCertificate needs.
    const again = der.reader(whole);
    print_int(der.peek_tag(again));
    const v = der.read_value(again) catch return 17;
    print(bytes.to_hex(der.encoded(v)));
    return 0;
}

/// The tag and length back in front of an OID's contents, so it can be
/// compared against a whole encoded one.
fn prefixed(oid: []u8) []u8 {
    const out = bytes.new(array.len(oid) + 2);
    out[0] = 0x06;
    out[1] = u8(array.len(oid));
    bytes.copy(out, 2, oid, 0, array.len(oid));
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
