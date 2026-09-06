// DER: reading the encoding X.509 is written in.
//
// Distinguished Encoding Rules are BER with the choices taken away, and taking
// them away is the whole reason this module is strict rather than forgiving.
// A certificate is a signed byte string: the signature covers the *encoding*,
// so any encoding this reader accepts but re-encodes differently is a
// signature that can be moved onto a different meaning. Two implementations
// disagreeing about what a byte string says is how a name gets confused for
// another one. So:
//
// - **Definite lengths only.** The indefinite form is BER and is refused.
// - **Minimal lengths.** 1 is `01`, never `81 01`, and a long form whose value
//   is below 128 is refused.
// - **Minimal integers.** A leading `00` is allowed only where the next byte
//   would otherwise be read as a sign bit.
// - **No trailing data.** `expect_end` is called, not assumed.
// - **No high tag numbers.** Nothing in X.509 uses a tag above 30, and the
//   multi-byte form is a parser for a case that does not arise.
//
// There is no writer. Nothing here produces DER: `std/x509` reads it, and
// `std/p256` reads an ECDSA signature out of it. A writer with no caller is
// what `std/cipher` decided about AES decryption.
//
// The cursor is a struct whose `at` moves, which is `http.Conn` and
// `list.Iter`'s shape -- and the one that lets a caller ask a nested value a
// question without copying the bytes out first.
const array = @import("std/array");
const bytes = @import("std/bytes");

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------
//
// The identifier octet, whole: class in the top two bits, constructed in bit
// 5, number in the bottom five. Written as the byte that appears rather than
// as three fields, because that is how they are compared.

pub const BOOLEAN = 0x01;
pub const INTEGER = 0x02;
pub const BIT_STRING = 0x03;
pub const OCTET_STRING = 0x04;
pub const NULL = 0x05;
pub const OID = 0x06;
pub const UTF8_STRING = 0x0c;
pub const PRINTABLE_STRING = 0x13;
pub const T61_STRING = 0x14;
pub const IA5_STRING = 0x16;
pub const UTC_TIME = 0x17;
pub const GENERALIZED_TIME = 0x18;
pub const SEQUENCE = 0x30;
pub const SET = 0x31;

/// The tag of `[n]`, the constructed context-specific form -- an X.509
/// `[0] EXPLICIT Version` or the `[3]` that holds the extensions.
pub fn context(n: i64) i64 { return 0xa0 | n; }

/// The tag of `[n]` in its primitive form, which is what a `GeneralName`'s
/// `dNSName` is.
pub fn context_prim(n: i64) i64 { return 0x80 | n; }

// ---------------------------------------------------------------------------
// The cursor
// ---------------------------------------------------------------------------

/// A position in a buffer, and where the value being read ends.
///
/// `end` rather than the buffer's length, because a nested value is read by
/// making a reader over its contents: the bound is the parent's, not the
/// file's, so a length that lies cannot reach past the value that declared it.
pub const Reader = struct { buf: []u8, at: i64, end: i64 };

/// One tag-length-value, located.
///
/// `head .. to` is the whole encoding and `from .. to` is the contents. Both
/// are wanted: a signature is computed over the whole of a TBSCertificate,
/// including its tag and its length, and everything else reads the contents.
pub const Value = struct { buf: []u8, tag: i64, head: i64, from: i64, to: i64 };

pub fn reader(b: []u8) Reader {
    return Reader{ .buf = b, .at = 0, .end = array.len(b) };
}

/// A reader over `b[from..to]`, clamped as `bytes.slice` is.
pub fn over(b: []u8, from: i64, to: i64) Reader {
    const n = array.len(b);
    var start = from;
    if (start < 0) { start = 0; }
    if (start > n) { start = n; }
    var stop = to;
    if (stop < start) { stop = start; }
    if (stop > n) { stop = n; }
    return Reader{ .buf = b, .at = start, .end = stop };
}

/// A reader over a value's contents.
pub fn body(v: Value) Reader {
    return Reader{ .buf = v.buf, .at = v.from, .end = v.to };
}

/// A value's contents, copied out.
pub fn raw(v: Value) []u8 { return bytes.slice(v.buf, v.from, v.to); }

/// A value's whole encoding, tag and length included, copied out.
pub fn encoded(v: Value) []u8 { return bytes.slice(v.buf, v.head, v.to); }

/// How many bytes are left.
pub fn left(r: Reader) i64 { return r.end - r.at; }

pub fn at_end(r: Reader) bool { return r.at >= r.end; }

/// Everything not yet read, copied out.
pub fn rest(r: Reader) []u8 { return bytes.slice(r.buf, r.at, r.end); }

/// The tag of the next value, or -1 if there is none.
///
/// Answering with a number rather than raising, because "is there an optional
/// field here?" is the question this exists for and a missing one is not an
/// error at the point it is asked.
pub fn peek_tag(r: Reader) i64 {
    if (r.at >= r.end) { return -1; }
    return i64(r.buf[r.at]);
}

pub fn has_tag(r: Reader, tag: i64) bool { return peek_tag(r) == tag; }

/// Refuse anything left over.
pub fn expect_end(r: Reader) !void {
    if (r.at != r.end) { return error.BadFormat; }
    return;
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// The next value, with the cursor moved past the whole of it.
pub fn read_value(r: Reader) !Value {
    if (r.at >= r.end) { return error.BadFormat; }
    const head = r.at;
    const tag = i64(r.buf[r.at]);
    // The five low bits all set is the multi-byte tag form. Nothing in a
    // certificate uses a tag above 30, so this is a shape that only ever
    // arrives from something that is not one.
    if (tag & 0x1f == 0x1f) { return error.BadFormat; }
    r.at += 1;
    if (r.at >= r.end) { return error.BadFormat; }
    const first = i64(r.buf[r.at]);
    r.at += 1;
    var length = 0;
    if (first < 0x80) {
        length = first;
    } else {
        // 0x80 is the indefinite form, which is BER; 0xff is reserved.
        const count = first & 0x7f;
        if (count == 0 or count > 4) { return error.BadFormat; }
        if (r.at + count > r.end) { return error.BadFormat; }
        // A leading zero would be a shorter encoding written long.
        if (r.buf[r.at] == 0) { return error.BadFormat; }
        var i = 0;
        while (i < count) : (i += 1) {
            length = (length << 8) | i64(r.buf[r.at + i]);
        }
        r.at += count;
        // ...and so would anything the short form could have said.
        if (length < 0x80) { return error.BadFormat; }
    }
    if (length < 0) { return error.BadFormat; }
    if (r.at + length > r.end) { return error.BadFormat; }
    const from = r.at;
    r.at += length;
    return Value{ .buf = r.buf, .tag = tag, .head = head, .from = from, .to = r.at };
}

/// The next value, which must carry `tag`, as a reader over its contents.
pub fn read_tagged(r: Reader, tag: i64) !Reader {
    const v = try read_value(r);
    if (v.tag != tag) { return error.BadFormat; }
    return body(v);
}

pub fn read_seq(r: Reader) !Reader { return read_tagged(r, SEQUENCE); }

pub fn read_set(r: Reader) !Reader { return read_tagged(r, SET); }

/// Step over the next value, whatever it is.
pub fn skip_value(r: Reader) !void {
    const v = try read_value(r);
    if (v.to < 0) { return error.BadFormat; }
    return;
}

/// The contents of the next INTEGER, checked for DER's minimal form.
///
/// Two's complement, so the bytes are signed: `00 80` is 128 and `80` is -128.
/// The minimality rule is what makes those the only spellings of each.
pub fn read_int_bytes(r: Reader) ![]u8 {
    const v = try read_value(r);
    if (v.tag != INTEGER) { return error.BadFormat; }
    const n = v.to - v.from;
    if (n == 0) { return error.BadFormat; }
    if (n > 1) {
        const a = i64(v.buf[v.from]);
        const b = i64(v.buf[v.from + 1]);
        if (a == 0x00 and b < 0x80) { return error.BadFormat; }
        if (a == 0xff and b >= 0x80) { return error.BadFormat; }
    }
    return raw(v);
}

/// The next INTEGER as unsigned big-endian bytes, with the sign byte removed.
///
/// A modulus, a serial and an ECDSA `r` are all non-negative integers that may
/// have a high bit set, so DER writes a leading `00` in front of them. That
/// byte is an artefact of the encoding rather than part of the number, and
/// every caller here wants the number.
pub fn read_uint_bytes(r: Reader) ![]u8 {
    const raw_bytes = try read_int_bytes(r);
    if (array.len(raw_bytes) == 0) { return error.BadFormat; }
    if (raw_bytes[0] >= 0x80) { return error.BadFormat; }
    if (array.len(raw_bytes) > 1 and raw_bytes[0] == 0) {
        return bytes.slice(raw_bytes, 1, array.len(raw_bytes));
    }
    return raw_bytes;
}

/// The next INTEGER as a number, for a version or a path length.
pub fn read_uint(r: Reader) !i64 {
    const b = try read_uint_bytes(r);
    const n = array.len(b);
    if (n > 8) { return error.BadFormat; }
    var v = 0;
    var i = 0;
    while (i < n) : (i += 1) { v = (v << 8) | i64(b[i]); }
    if (v < 0) { return error.BadFormat; }
    return v;
}

pub fn read_bool(r: Reader) !bool {
    const v = try read_value(r);
    if (v.tag != BOOLEAN) { return error.BadFormat; }
    if (v.to - v.from != 1) { return error.BadFormat; }
    // DER has exactly two booleans; BER has 255 trues.
    const b = v.buf[v.from];
    if (b == 0) { return false; }
    if (b == 0xff) { return true; }
    return error.BadFormat;
}

/// The next OBJECT IDENTIFIER's contents, undecoded.
///
/// Left as bytes because every use is a comparison against a known one, and
/// the dotted form nothing prints is a decoder with no reader.
pub fn read_oid(r: Reader) ![]u8 {
    const v = try read_value(r);
    if (v.tag != OID) { return error.BadFormat; }
    if (v.to == v.from) { return error.BadFormat; }
    return raw(v);
}

/// The next BIT STRING's bits, which must be a whole number of bytes.
///
/// A public key and a signature are both carried in one, and both are byte
/// strings, so a non-zero unused-bit count is a certificate that means
/// something this cannot represent rather than one to guess about.
pub fn read_bitstring(r: Reader) ![]u8 {
    const v = try read_value(r);
    if (v.tag != BIT_STRING) { return error.BadFormat; }
    if (v.to == v.from) { return error.BadFormat; }
    if (v.buf[v.from] != 0) { return error.BadFormat; }
    return bytes.slice(v.buf, v.from + 1, v.to);
}

pub fn read_octets(r: Reader) ![]u8 {
    const v = try read_value(r);
    if (v.tag != OCTET_STRING) { return error.BadFormat; }
    return raw(v);
}

/// Read and discard a NULL, which is how the RSA algorithm identifiers write
/// their absent parameters.
pub fn read_null(r: Reader) !void {
    const v = try read_value(r);
    if (v.tag != NULL) { return error.BadFormat; }
    if (v.to != v.from) { return error.BadFormat; }
    return;
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------
//
// A certificate says when it stops being one, and `std/time.now` says what
// time it is, so the two have to meet in the same unit. That unit is seconds
// since the Unix epoch, which means this module owns a calendar.
//
// DER pins the format down to one spelling each: four digits of seconds, no
// fraction, and a literal `Z`. RFC 5280 section 4.1.2.5 then adds the rule
// that decides which century a two-digit year is in.

fn digits(b: []u8, at: i64, n: i64) !i64 {
    var v = 0;
    var i = 0;
    while (i < n) : (i += 1) {
        const c = i64(b[at + i]);
        if (c < 48 or c > 57) { return error.BadFormat; }
        v = v * 10 + (c - 48);
    }
    return v;
}

/// Days from 1970-01-01 to `y-m-d`, by Howard Hinnant's civil-from-days.
///
/// Written out rather than looped over years, because a certificate may be
/// valid until 9999 and a loop would be a thousand iterations to answer a
/// question that is three divisions. Correct for `y >= 0`, which every year a
/// certificate can name is.
fn days_from_civil(y0: i64, m: i64, d: i64) i64 {
    var y = y0;
    if (m <= 2) { y -= 1; }
    const era = y / 400;
    const yoe = y - era * 400;
    const mp = (m + 9) % 12;
    const doy = (153 * mp + 2) / 5 + d - 1;
    const doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + doe - 719468;
}

const DAYS_IN_MONTH = []i64{ 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 };

fn leap(y: i64) bool {
    if (y % 4 != 0) { return false; }
    if (y % 100 != 0) { return true; }
    return y % 400 == 0;
}

/// The next UTCTime or GeneralizedTime, as seconds since the Unix epoch.
pub fn read_time(r: Reader) !i64 {
    const v = try read_value(r);
    const n = v.to - v.from;
    var year = 0;
    var at = v.from;
    if (v.tag == UTC_TIME) {
        if (n != 13) { return error.BadFormat; }
        const yy = try digits(v.buf, at, 2);
        // RFC 5280: 50 and above is the twentieth century.
        if (yy < 50) { year = 2000 + yy; } else { year = 1900 + yy; }
        at += 2;
    } else {
        if (v.tag != GENERALIZED_TIME) { return error.BadFormat; }
        if (n != 15) { return error.BadFormat; }
        year = try digits(v.buf, at, 4);
        at += 4;
    }
    if (v.buf[v.to - 1] != 0x5a) { return error.BadFormat; }
    const mon = try digits(v.buf, at, 2);
    const day = try digits(v.buf, at + 2, 2);
    const hour = try digits(v.buf, at + 4, 2);
    const min = try digits(v.buf, at + 6, 2);
    const sec = try digits(v.buf, at + 8, 2);
    if (mon < 1 or mon > 12) { return error.BadFormat; }
    var last = DAYS_IN_MONTH[mon - 1];
    if (mon == 2 and leap(year)) { last = 29; }
    if (day < 1 or day > last) { return error.BadFormat; }
    if (hour > 23 or min > 59) { return error.BadFormat; }
    // 60 is a leap second, which DER permits and which no certificate needs.
    if (sec > 59) { return error.BadFormat; }
    return days_from_civil(year, mon, day) * 86400 + hour * 3600 + min * 60 + sec;
}
