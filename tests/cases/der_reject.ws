// Every way a DER encoding can fail to be one, a line each.
//
// This is the file where being strict is the feature. A certificate is a
// signed byte string, so any encoding this accepts but a second implementation
// re-encodes differently is a signature that has been moved onto a different
// meaning -- and the historical X.509 failures are all *acceptances*: a length
// read leniently, an integer with a spare leading zero, a BER indefinite form
// let through by a parser written from the BER spec.
//
// So each refusal is its own line, the way the AEADs' and the RSA verifier's
// are. A single "malformed input is refused" check would pass with any one of
// these accepted.
// expect: a well-formed value is still read
// expect: an indefinite length refused
// expect: a long form where the short one would do refused
// expect: a length with a leading zero byte refused
// expect: a length past the end of the buffer refused
// expect: an integer with a spare leading zero refused
// expect: a negative integer where unsigned is meant refused
// expect: an empty integer refused
// expect: a bit string with unused bits refused
// expect: a multi-byte tag refused
// expect: one trailing byte refused
// expect: a boolean that is neither 0 nor 255 refused
// expect: a time with no Z refused
// expect: a thirteenth month refused
// expect: the thirty-first of February refused
// expect: a time missing its seconds refused
// expect: an integer too wide for the answer refused
const der = @import("std/der");
const bytes = @import("std/bytes");

const GOOD = "300302010f";
const BAD_INDEFINITE = "30800201010000";
const BAD_LONG_NONMINIMAL = "048101aa";
const BAD_LEADING_ZERO_LEN = "048200800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
const BAD_LEN_PAST_END = "0410aabb";
const BAD_INT_LEADING_ZERO = "300402020001";
const BAD_INT_NEGATIVE = "30030201ff";
const BAD_INT_EMPTY = "30020200";
const BAD_BITSTRING_UNUSED = "030203f8";
const BAD_HIGH_TAG = "1f810001aa";
const BAD_TRAILING = "300302010100";
const BAD_BOOL = "010101";
const BAD_UTC_NO_Z = "170d3236303930363132303030302b";
const BAD_UTC_MONTH = "170d3236313330363132303030305a";
const BAD_UTC_DAY = "170d3236303233313132303030305a";
const BAD_UTC_SHORT = "170b323630393036313230305a";
/// Nine bytes of INTEGER, which is a number `read_uint` cannot answer with.
const BAD_INT_WIDE = "300b0209010000000000000000";

fn main() i64 {
    // The line that keeps the rest honest: a reader that refuses everything
    // passes every refusal below.
    const ok = der.reader(hex(GOOD));
    const seq = der.read_seq(ok) catch return 1;
    if ((der.read_uint(seq) catch return 2) == 15) {
        print("a well-formed value is still read");
    }

    seq_refused(BAD_INDEFINITE, "an indefinite length refused");
    octets_refused(BAD_LONG_NONMINIMAL, "a long form where the short one would do refused");
    octets_refused(BAD_LEADING_ZERO_LEN, "a length with a leading zero byte refused");
    octets_refused(BAD_LEN_PAST_END, "a length past the end of the buffer refused");
    uint_refused(BAD_INT_LEADING_ZERO, "an integer with a spare leading zero refused");
    uint_refused(BAD_INT_NEGATIVE, "a negative integer where unsigned is meant refused");
    uint_refused(BAD_INT_EMPTY, "an empty integer refused");
    bitstring_refused(BAD_BITSTRING_UNUSED, "a bit string with unused bits refused");
    octets_refused(BAD_HIGH_TAG, "a multi-byte tag refused");
    trailing_refused(BAD_TRAILING, "one trailing byte refused");
    bool_refused(BAD_BOOL, "a boolean that is neither 0 nor 255 refused");
    time_refused(BAD_UTC_NO_Z, "a time with no Z refused");
    time_refused(BAD_UTC_MONTH, "a thirteenth month refused");
    time_refused(BAD_UTC_DAY, "the thirty-first of February refused");
    time_refused(BAD_UTC_SHORT, "a time missing its seconds refused");
    uint_refused(BAD_INT_WIDE, "an integer too wide for the answer refused");
    return 0;
}

fn seq_refused(s: str, note: str) void {
    der.read_seq(der.reader(hex(s))) catch { print(note); return; };
    return;
}

fn octets_refused(s: str, note: str) void {
    der.read_octets(der.reader(hex(s))) catch { print(note); return; };
    return;
}

fn bitstring_refused(s: str, note: str) void {
    der.read_bitstring(der.reader(hex(s))) catch { print(note); return; };
    return;
}

fn bool_refused(s: str, note: str) void {
    der.read_bool(der.reader(hex(s))) catch { print(note); return; };
    return;
}

fn time_refused(s: str, note: str) void {
    der.read_time(der.reader(hex(s))) catch { print(note); return; };
    return;
}

/// An unsigned integer inside a SEQUENCE, which is where every one in a
/// certificate lives.
fn uint_refused(s: str, note: str) void {
    const r = der.reader(hex(s));
    const seq = der.read_seq(r) catch { print("unreachable"); return; };
    der.read_uint(seq) catch { print(note); return; };
    return;
}

/// A whole value that parses, followed by a byte that is not part of it.
fn trailing_refused(s: str, note: str) void {
    const r = der.reader(hex(s));
    der.read_seq(r) catch { print("unreachable"); return; };
    der.expect_end(r) catch { print(note); return; };
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
