// DEFLATE, and the zlib wrapper over it. RFC 1951 and RFC 1950.
//
// Here because a git packfile is a concatenation of zlib streams, which is also
// the reason the interface is a **cursor rather than a one-shot**: the reader
// has to answer with how many *input* bytes it consumed, or the caller cannot
// find where the next object begins. A `decompress(bytes) -> bytes` would be
// the obvious shape and would be useless for the one caller there is.
//
// The decoder is Mark Adler's `puff` in outline: canonical Huffman decoded from
// a table of counts and a table of symbols, rather than from a tree. That is
// not only smaller -- it is what lets a block cost no allocation at all, which
// matters because the whole case suite runs a second time under `--gc-stress`.
// Both tables live in the state and are made once.
//
// There is no `crc32` and no gzip wrapper. zlib checks with Adler-32 and that
// is what a packfile carries; a writer -- or a checksum -- with no caller is
// what `std/der` decided about DER and `std/cipher` about AES decryption.
const array = @import("std/array");
const bits = @import("std/bits");
const bytes = @import("std/bytes");

/// The longest a DEFLATE code can be.
const MAX_BITS = 15;
/// Literal/length alphabet: 0..255 literals, 256 end-of-block, 257..285 lengths.
const LIT_SYMBOLS = 288;
const DIST_SYMBOLS = 30;
const CODE_SYMBOLS = 19;

/// Base length for code 257 upwards, and how many extra bits follow.
const LENGTH_BASE = []i64{
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
    35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
};
const LENGTH_EXTRA = []i64{
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
    3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
};
const DIST_BASE = []i64{
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
    257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
};
const DIST_EXTRA = []i64{
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
    7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
};
/// The order the code-length code lengths are written in, which is chosen so
/// that a stream can stop early and leave the rest implicitly zero.
const CODE_ORDER = []i64{
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
};

/// A reader over one stream, and the tables it decodes with.
///
/// Made once and reused: a `Reader` per stream would be fine, but the tables
/// inside it are what a block would otherwise allocate.
pub const Reader = struct {
    src: []u8,
    at: i64,
    end: i64,
    /// Bits waiting to be read, least significant first, and how many.
    bitbuf: i64,
    bitcnt: i64,
    out: bytes.Buf,

    lencnt: []i64,
    lensym: []i64,
    distcnt: []i64,
    distsym: []i64,
    /// Scratch for the code lengths a dynamic block writes down.
    lengths: []i64,

    ok: bool,
};

pub fn reader(src: []u8, at: i64, out: bytes.Buf) Reader {
    return Reader{
        .src = src,
        .at = at,
        .end = array.len(src),
        .bitbuf = 0,
        .bitcnt = 0,
        .out = out,
        .lencnt = array.new(MAX_BITS + 1),
        .lensym = array.new(LIT_SYMBOLS),
        .distcnt = array.new(MAX_BITS + 1),
        .distsym = array.new(DIST_SYMBOLS + 2),
        .lengths = array.new(LIT_SYMBOLS + DIST_SYMBOLS + 2),
        .ok = true,
    };
}

/// Read a zlib stream at `at`, appending what it holds to `out`.
///
/// Answers with the offset just past the stream, which is what lets a caller
/// walk a file of them. The Adler-32 trailer is checked: a packfile that has
/// been truncated or corrupted in transit should say so here rather than
/// three layers up as a nonsensical object.
pub fn zlib(src: []u8, at: i64, out: bytes.Buf) !{BadFormat}i64 {
    if (at + 2 > array.len(src)) { return error.BadFormat; }
    const cmf = i64(src[at]);
    const flg = i64(src[at + 1]);
    // Deflate, a window no larger than 32 KiB, no preset dictionary, and the
    // two header bytes a multiple of 31.
    if (cmf & 15 != 8) { return error.BadFormat; }
    if ((cmf >> 4) > 7) { return error.BadFormat; }
    if (flg & 32 != 0) { return error.BadFormat; }
    if ((cmf * 256 + flg) % 31 != 0) { return error.BadFormat; }

    const start = out.used;
    const r = reader(src, at + 2, out);
    const consumed = try inflate(r);
    if (consumed + 4 > array.len(src)) { return error.BadFormat; }
    const want = bytes.be32(src, consumed);
    if (adler32(out.data, start, out.used - start) != want) { return error.BadFormat; }
    return consumed + 4;
}

/// The same without the zlib wrapper: raw DEFLATE, and no checksum to check.
pub fn raw(src: []u8, at: i64, out: bytes.Buf) !{BadFormat}i64 {
    return inflate(reader(src, at, out));
}

/// Every block until the last one, and the offset just past them.
fn inflate(r: Reader) !{BadFormat}i64 {
    var last = 0;
    while (last == 0) {
        last = take(r, 1);
        const kind = take(r, 2);
        if (kind == 0) { stored(r); }
        else if (kind == 1) { fixed(r); }
        else if (kind == 2) { dynamic(r); }
        else { fail(r); }
        if (!r.ok) { return error.BadFormat; }
    }
    // Whatever is left of the byte the last block ended in is not the next
    // stream's.
    return r.at;
}

fn fail(r: Reader) void { r.ok = false; return; }

/// `n` bits, least significant first, which is the order DEFLATE writes them.
///
/// Past the end answers with zeros and marks the reader broken, so a truncated
/// stream stops rather than reading whatever is next in memory.
fn take(r: Reader, n: i64) i64 {
    while (r.bitcnt < n) {
        if (r.at >= r.end) { fail(r); return 0; }
        r.bitbuf = r.bitbuf | (i64(r.src[r.at]) << r.bitcnt);
        r.at += 1;
        r.bitcnt += 8;
    }
    const v = r.bitbuf & ((1 << n) - 1);
    r.bitbuf = r.bitbuf >> n;
    r.bitcnt -= n;
    return v;
}

/// A block that is not compressed at all: a length, its complement, and bytes.
fn stored(r: Reader) void {
    // The length is byte-aligned, so whatever is left of the current byte goes.
    r.bitbuf = 0;
    r.bitcnt = 0;
    if (r.at + 4 > r.end) { fail(r); return; }
    const n = i64(r.src[r.at]) | (i64(r.src[r.at + 1]) << 8);
    const check = i64(r.src[r.at + 2]) | (i64(r.src[r.at + 3]) << 8);
    if (n != (check ^ 0xffff)) { fail(r); return; }
    r.at += 4;
    if (r.at + n > r.end) { fail(r); return; }
    bytes.put_bytes(r.out, r.src, r.at, n);
    r.at += n;
    return;
}

/// The block type whose code lengths are fixed by the specification.
///
/// Written into the same tables a dynamic block uses, rather than kept beside
/// them: one decoder, and one place for a table to be wrong.
fn fixed(r: Reader) void {
    var i = 0;
    while (i < 144) : (i += 1) { r.lengths[i] = 8; }
    while (i < 256) : (i += 1) { r.lengths[i] = 9; }
    while (i < 280) : (i += 1) { r.lengths[i] = 7; }
    while (i < 288) : (i += 1) { r.lengths[i] = 8; }
    construct(r, r.lencnt, r.lensym, 0, 288);
    i = 0;
    while (i < 30) : (i += 1) { r.lengths[i] = 5; }
    construct(r, r.distcnt, r.distsym, 0, 30);
    if (!r.ok) { return; }
    codes(r);
    return;
}

/// The block type that writes its own code lengths down first.
fn dynamic(r: Reader) void {
    const nlen = take(r, 5) + 257;
    const ndist = take(r, 5) + 1;
    const ncode = take(r, 4) + 4;
    if (!r.ok) { return; }
    if (nlen > LIT_SYMBOLS or ndist > DIST_SYMBOLS + 2) { fail(r); return; }

    // The code lengths of the code-length code, in their own peculiar order.
    var i = 0;
    while (i < CODE_SYMBOLS) : (i += 1) { r.lengths[i] = 0; }
    i = 0;
    while (i < ncode) : (i += 1) { r.lengths[CODE_ORDER[i]] = take(r, 3); }
    if (!r.ok) { return; }
    construct(r, r.lencnt, r.lensym, 0, CODE_SYMBOLS);
    if (!r.ok) { return; }

    // Then the real code lengths, run-length encoded by that code.
    var written = 0;
    while (written < nlen + ndist) {
        const symbol = decode(r, r.lencnt, r.lensym);
        if (!r.ok) { return; }
        if (symbol < 16) {
            r.lengths[written] = symbol;
            written += 1;
            continue;
        }
        var repeat = 0;
        var value = 0;
        if (symbol == 16) {
            if (written == 0) { fail(r); return; }
            value = r.lengths[written - 1];
            repeat = 3 + take(r, 2);
        } else {
            if (symbol == 17) { repeat = 3 + take(r, 3); } else { repeat = 11 + take(r, 7); }
        }
        if (!r.ok) { return; }
        if (written + repeat > nlen + ndist) { fail(r); return; }
        var k = 0;
        while (k < repeat) : (k += 1) {
            r.lengths[written] = value;
            written += 1;
        }
    }
    // A block with no end-of-block code could never end.
    if (r.lengths[256] == 0) { fail(r); return; }

    construct(r, r.lencnt, r.lensym, 0, nlen);
    if (!r.ok) { return; }
    construct(r, r.distcnt, r.distsym, nlen, ndist);
    if (!r.ok) { return; }
    codes(r);
    return;
}

/// Literals and back-references until the end-of-block symbol.
fn codes(r: Reader) void {
    var going = true;
    while (going) {
        const symbol = decode(r, r.lencnt, r.lensym);
        if (!r.ok) { return; }
        if (symbol < 256) { bytes.put_u8(r.out, symbol); continue; }
        if (symbol == 256) { going = false; continue; }
        const which = symbol - 257;
        if (which >= 29) { fail(r); return; }
        const length = LENGTH_BASE[which] + take(r, LENGTH_EXTRA[which]);

        const d = decode(r, r.distcnt, r.distsym);
        if (!r.ok) { return; }
        if (d >= 30) { fail(r); return; }
        const distance = DIST_BASE[d] + take(r, DIST_EXTRA[d]);
        if (!r.ok) { return; }
        if (distance > r.out.used) { fail(r); return; }

        // One byte at a time, and not a block move: a distance shorter than
        // the length is legal and is how a run is written, so the bytes being
        // copied are partly the bytes being written.
        var k = 0;
        while (k < length) : (k += 1) {
            bytes.put_u8(r.out, i64(r.out.data[r.out.used - distance]));
        }
    }
    return;
}

/// One symbol, walking the code lengths from short to long.
///
/// Canonical Huffman needs no tree: for each length, the codes of that length
/// are consecutive, so `code - first` is an index into the symbols of that
/// length and `index` says where those begin.
fn decode(r: Reader, counts: []i64, symbols: []i64) i64 {
    var code = 0;
    var first = 0;
    var index = 0;
    var length = 1;
    while (length <= MAX_BITS) : (length += 1) {
        code = code | take(r, 1);
        if (!r.ok) { return 0; }
        const count = counts[length];
        if (code - first < count) { return symbols[index + (code - first)]; }
        index += count;
        first = (first + count) << 1;
        code = code << 1;
    }
    fail(r);
    return 0;
}

/// Build the counts and symbols of a canonical Huffman code from `n` code
/// lengths starting at `from`.
///
/// A code that is *over-subscribed* -- more codes than the lengths allow -- is
/// refused. An incomplete one is allowed only where the specification allows
/// it: a distance code with a single symbol, which a stream that never
/// references anything writes.
fn construct(r: Reader, counts: []i64, symbols: []i64, from: i64, n: i64) void {
    var i = 0;
    while (i <= MAX_BITS) : (i += 1) { counts[i] = 0; }
    i = 0;
    while (i < n) : (i += 1) {
        const length = r.lengths[from + i];
        if (length < 0 or length > MAX_BITS) { fail(r); return; }
        counts[length] += 1;
    }
    if (counts[0] == n) { return; }

    var left = 1;
    var length = 1;
    while (length <= MAX_BITS) : (length += 1) {
        left = left << 1;
        left -= counts[length];
        if (left < 0) { fail(r); return; }
    }

    // Where the symbols of each length begin.
    const offsets = array.new(MAX_BITS + 2);
    offsets[1] = 0;
    length = 1;
    while (length <= MAX_BITS) : (length += 1) {
        offsets[length + 1] = offsets[length] + counts[length];
    }
    i = 0;
    while (i < n) : (i += 1) {
        const at = r.lengths[from + i];
        if (at != 0) {
            symbols[offsets[at]] = i;
            offsets[at] += 1;
        }
    }
    return;
}

/// Adler-32 of `n` bytes at `at`, which is what a zlib stream ends with.
pub fn adler32(b: []u8, at: i64, n: i64) u32 {
    // 65521, the largest prime below 2^16.
    var a: u64 = 1;
    var c: u64 = 0;
    var i = 0;
    while (i < n) : (i += 1) {
        a += u64(b[at + i]);
        c += a;
        // Reducing every 4096 bytes rather than every byte: the sums cannot
        // overflow a `u64` in that span, and a modulo per byte is most of the
        // cost of the whole function.
        if (i % 4096 == 4095) {
            a = a % 65521;
            c = c % 65521;
        }
    }
    a = a % 65521;
    c = c % 65521;
    return u32((c << 16) | a);
}
