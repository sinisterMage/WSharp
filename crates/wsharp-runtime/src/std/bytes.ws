// Byte buffers, and the bridge between `[]u8` and `str`.
//
// A `str` in W# is arbitrary bytes rather than text, so it has always been a
// fine *container*; what it is not is a buffer. It is immutable, so a hash
// assembling its message a block at a time through `str.concat` is quadratic,
// and item 10's ciphers work in place by construction. A `[]u8` is the buffer,
// and this module is what gets bytes into and out of one.
//
// The split between here and Rust is the usual one, with one clause added.
// `raw_to_str`, `raw_from_str`, `copy` and `equal` are builtins because they
// are bulk moves of *bytes*, which a builtin may do. They do not *allocate*:
// `array.new` is lowered inline, because only the call site knows the element
// type and so the stride and the type id to stamp, which leaves the allocation
// W#'s in every case. Everything else here is a loop that would be no faster
// in Rust.
//
// The word accessors are the other half of the module's job. Every hash and
// cipher in `std/hash` and `std/cipher` is spelled in whole words loaded from
// and stored to a byte buffer, and which end they load from is half of what
// distinguishes SHA-256 from ChaCha20.
const array = @import("std/array");
const text = @import("std/str");

/// A zeroed buffer of `n` bytes.
///
/// Here rather than left to `array.new` so that the annotation `array.new`
/// needs -- the element type comes from unification, not from an argument --
/// is written once instead of at every call.
pub fn new(n: i64) []u8 {
    const out: []u8 = array.new(n);
    return out;
}

/// The bytes of `s`, as a buffer that can be written.
pub fn of(s: str) []u8 {
    const out = new(text.len(s));
    raw_from_str(out, 0, s);
    return out;
}

/// Every byte of `b`, as a `str` -- which is what a socket takes.
pub fn to_str(b: []u8) str {
    return raw_to_str(b, 0, array.len(b));
}

/// `b[from..to]` as a `str`, clamped to the buffer rather than panicking.
pub fn slice_str(b: []u8, from: i64, to: i64) str {
    return raw_to_str(b, from, to);
}

/// `b[from..to]` as a buffer of its own.
pub fn slice(b: []u8, from: i64, to: i64) []u8 {
    var start = from;
    if (start < 0) { start = 0; }
    if (start > array.len(b)) { start = array.len(b); }
    var end = to;
    if (end < start) { end = start; }
    if (end > array.len(b)) { end = array.len(b); }
    const out = new(end - start);
    copy(out, 0, b, start, end - start);
    return out;
}

/// `a` followed by `b`.
pub fn concat(a: []u8, b: []u8) []u8 {
    const out = new(array.len(a) + array.len(b));
    copy(out, 0, a, 0, array.len(a));
    copy(out, array.len(a), b, 0, array.len(b));
    return out;
}

/// Write `v` into `n` bytes of `b` from `at`.
///
/// A loop rather than a builtin: HMAC's two pads are the only callers that
/// care, and 128 bytes of loop is not worth a row in the table.
pub fn fill(b: []u8, at: i64, n: i64, v: u8) void {
    var i = 0;
    while (i < n) : (i += 1) { b[at + i] = v; }
    return;
}

/// `a[at..at+n] ^= b[from..from+n]`, which is what every mode of operation and
/// every padding step in `std/hash` is written in terms of.
pub fn xor(a: []u8, at: i64, b: []u8, from: i64, n: i64) void {
    var i = 0;
    while (i < n) : (i += 1) { a[at + i] ^= b[from + i]; }
    return;
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------
//
// Loaded and stored a byte at a time rather than as a machine word, and
// deliberately: a buffer offset has no alignment to promise, and every load
// and store W# emits through `layout`'s offsets carries Cranelift's `aligned`
// flag, which lets the instruction "trap or return a wrong result" if the
// address is not aligned. Assembling the word by hand is what keeps that flag
// honest, and it is what makes the byte order the program's choice rather
// than the machine's.

/// The big-endian `u32` at `at` -- SHA-2's word order, and the wire's.
pub fn be32(b: []u8, at: i64) u32 {
    return (u32(b[at]) << 24) | (u32(b[at + 1]) << 16)
         | (u32(b[at + 2]) << 8) | u32(b[at + 3]);
}

pub fn put_be32(b: []u8, at: i64, v: u32) void {
    b[at] = u8(v >> 24);
    b[at + 1] = u8(v >> 16);
    b[at + 2] = u8(v >> 8);
    b[at + 3] = u8(v);
    return;
}

pub fn be64(b: []u8, at: i64) u64 {
    return (u64(be32(b, at)) << 32) | u64(be32(b, at + 4));
}

pub fn put_be64(b: []u8, at: i64, v: u64) void {
    put_be32(b, at, u32(v >> 32));
    put_be32(b, at + 4, u32(v));
    return;
}

/// The big-endian 16- and 24-bit integers at `at`.
///
/// Answered as an `i64` rather than as a `u16`, which is a deviation from the
/// accessors above and a deliberate one: every one of these in TLS and in DER
/// is a *length*, an index into a buffer is an `i64` in this language, and a
/// `u16` here would put a conversion at every single use. There is no 24-bit
/// type to be faithful to in the second case anyway.
pub fn be16(b: []u8, at: i64) i64 {
    return (i64(b[at]) << 8) | i64(b[at + 1]);
}

pub fn put_be16(b: []u8, at: i64, v: i64) void {
    b[at] = u8(v >> 8);
    b[at + 1] = u8(v);
    return;
}

pub fn be24(b: []u8, at: i64) i64 {
    return (i64(b[at]) << 16) | (i64(b[at + 1]) << 8) | i64(b[at + 2]);
}

pub fn put_be24(b: []u8, at: i64, v: i64) void {
    b[at] = u8(v >> 16);
    b[at + 1] = u8(v >> 8);
    b[at + 2] = u8(v);
    return;
}

/// The little-endian `u32` at `at` -- ChaCha20's word order, and Poly1305's.
pub fn le32(b: []u8, at: i64) u32 {
    return u32(b[at]) | (u32(b[at + 1]) << 8)
         | (u32(b[at + 2]) << 16) | (u32(b[at + 3]) << 24);
}

pub fn put_le32(b: []u8, at: i64, v: u32) void {
    b[at] = u8(v);
    b[at + 1] = u8(v >> 8);
    b[at + 2] = u8(v >> 16);
    b[at + 3] = u8(v >> 24);
    return;
}

pub fn le64(b: []u8, at: i64) u64 {
    return u64(le32(b, at)) | (u64(le32(b, at + 4)) << 32);
}

pub fn put_le64(b: []u8, at: i64, v: u64) void {
    put_le32(b, at, u32(v));
    put_le32(b, at + 4, u32(v >> 32));
    return;
}

// ---------------------------------------------------------------------------
// A growable buffer
// ---------------------------------------------------------------------------
//
// `List[T]` is the growable array and would serve, but a `List[u8]` is a call
// per byte and its backing store is not a `[]u8` anything else here accepts.
// This is the same idea specialised: a `[]u8` whose header length is the
// capacity and a `used` beside it, so `taken` hands back exactly what was
// written and every builtin above can be pointed straight at `data`.
//
// The reason it exists at all is length prefixes. Every message in TLS and
// every value in DER is written `length, then contents`, and the length is not
// known until the contents are. `open16` writes a placeholder and answers with
// where it went; `close16` goes back and fills it in. Doing that against an
// immutable `str` would mean assembling the contents separately and
// concatenating, which is the quadratic shape `to_hex` above already avoids.

pub const Buf = struct { data: []u8, used: i64 };

/// A buffer that can hold `capacity` bytes before it has to grow.
pub fn buf(capacity: i64) Buf {
    var n = capacity;
    if (n < 16) { n = 16; }
    return Buf{ .data = new(n), .used = 0 };
}

/// Make room for `extra` more bytes, doubling as `std/list` does.
fn reserve(b: Buf, extra: i64) void {
    const need = b.used + extra;
    if (need <= array.len(b.data)) { return; }
    var cap = array.len(b.data);
    while (cap < need) : (cap *= 2) { }
    const bigger = new(cap);
    copy(bigger, 0, b.data, 0, b.used);
    b.data = bigger;
    return;
}

pub fn put_u8(b: Buf, v: i64) void {
    reserve(b, 1);
    b.data[b.used] = u8(v);
    b.used += 1;
    return;
}

pub fn put_u16(b: Buf, v: i64) void {
    reserve(b, 2);
    put_be16(b.data, b.used, v);
    b.used += 2;
    return;
}

pub fn put_u24(b: Buf, v: i64) void {
    reserve(b, 3);
    put_be24(b.data, b.used, v);
    b.used += 3;
    return;
}

pub fn put_u32(b: Buf, v: u32) void {
    reserve(b, 4);
    put_be32(b.data, b.used, v);
    b.used += 4;
    return;
}

/// Append `src[at..at+n]`.
pub fn put_bytes(b: Buf, src: []u8, at: i64, n: i64) void {
    reserve(b, n);
    copy(b.data, b.used, src, at, n);
    b.used += n;
    return;
}

/// Append the whole of `src`.
pub fn put_all(b: Buf, src: []u8) void {
    put_bytes(b, src, 0, array.len(src));
    return;
}

/// Append the bytes of `s`.
pub fn put_str(b: Buf, s: str) void {
    const n = text.len(s);
    reserve(b, n);
    raw_from_str(b.data, b.used, s);
    b.used += n;
    return;
}

/// Write `n` zero bytes as a placeholder for a length, and answer with where
/// they went. `close8`, `close16` and `close24` fill one in.
///
/// Three widths rather than one taking a parameter, because TLS uses all three
/// and a `close` that had to be told the width again is a `close` that can be
/// told the wrong one.
pub fn open8(b: Buf) i64 { put_u8(b, 0); return b.used - 1; }

pub fn close8(b: Buf, mark: i64) void {
    b.data[mark] = u8(b.used - mark - 1);
    return;
}

pub fn open16(b: Buf) i64 { put_u16(b, 0); return b.used - 2; }

pub fn close16(b: Buf, mark: i64) void {
    put_be16(b.data, mark, b.used - mark - 2);
    return;
}

pub fn open24(b: Buf) i64 { put_u24(b, 0); return b.used - 3; }

pub fn close24(b: Buf, mark: i64) void {
    put_be24(b.data, mark, b.used - mark - 3);
    return;
}

/// Everything written so far, as a buffer of its own.
pub fn taken(b: Buf) []u8 { return slice(b.data, 0, b.used); }

/// Forget everything written, keeping the capacity.
pub fn reset(b: Buf) void {
    b.used = 0;
    return;
}

/// Forget the first `n` bytes and shift the rest down.
///
/// What a record reader does with the bytes it has consumed. `copy` is a
/// memmove, so the overlap is safe.
pub fn drop_front(b: Buf, n: i64) void {
    var k = n;
    if (k < 0) { k = 0; }
    if (k > b.used) { k = b.used; }
    copy(b.data, 0, b.data, k, b.used - k);
    b.used -= k;
    return;
}

// ---------------------------------------------------------------------------
// Hexadecimal
// ---------------------------------------------------------------------------
//
// Here rather than in a test helper because every published test vector for
// every primitive in item 10 is written in hex, and because a string literal
// has no `\x` escape -- so hex is the only way to write a byte string down at
// all. `str.from_int` is decimal and `str.parse_int` is base ten only.

const DIGITS = []u8{
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57,  // '0'..'9'
    97, 98, 99, 100, 101, 102,               // 'a'..'f'
};

/// `b` as lowercase hex.
///
/// Built into a buffer and converted once, rather than concatenated: `concat`
/// on a `str` copies the accumulator, so the obvious spelling is quadratic and
/// this one is not.
pub fn to_hex(b: []u8) str {
    const out = new(array.len(b) * 2);
    var i = 0;
    while (i < array.len(b)) : (i += 1) {
        out[i * 2] = DIGITS[i64(b[i] >> 4)];
        out[i * 2 + 1] = DIGITS[i64(b[i] & 0x0f)];
    }
    return to_str(out);
}

/// The bytes a hex string spells.
///
/// Strict, for `str.parse_int`'s reason: an odd length or a digit that is not
/// one is `error.BadFormat` rather than a partial answer. A parser that wants
/// to be lenient can be; one that wants to be strict cannot recover leniency.
pub fn from_hex(s: str) ![]u8 {
    if (text.len(s) % 2 != 0) { return error.BadFormat; }
    const out = new(text.len(s) / 2);
    var i = 0;
    while (i < array.len(out)) : (i += 1) {
        const hi = try nibble(text.byte_at(s, i * 2));
        const lo = try nibble(text.byte_at(s, i * 2 + 1));
        out[i] = u8(hi * 16 + lo);
    }
    return out;
}

fn nibble(c: i64) !i64 {
    if (c >= 48 and c <= 57) { return c - 48; }
    if (c >= 97 and c <= 102) { return c - 87; }
    if (c >= 65 and c <= 70) { return c - 55; }
    return error.BadFormat;
}
