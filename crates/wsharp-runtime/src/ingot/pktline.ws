// pkt-line, which is how git frames everything it says over a socket.
//
// Four hexadecimal digits give the length of the line *including* those four
// bytes, so 0005 is a line with one byte in it and 0004 is an empty one. Three
// lengths are not lengths at all:
//
//   * `0000` -- flush: the end of a section, or of the conversation.
//   * `0001` -- delimiter: the end of a command's arguments, in protocol v2.
//   * `0002` -- response end, which only the stateless variants send.
//
// A line's payload usually ends in a newline and the newline is part of it,
// which is why `trimmed` exists: every caller wants the text without it and
// none of them wants to think about whether it was there.
const array = @import("std/array");
const bytes = @import("std/bytes");
const text = @import("std/str");

pub const DATA = 0;
pub const FLUSH = 1;
pub const DELIM = 2;
pub const RESPONSE_END = 3;
/// The stream is malformed, or ends in the middle of a line.
pub const BROKEN = 4;

/// One line, located rather than copied: `from .. to` is its payload and
/// `next` is where the line after it begins.
pub const Pkt = struct { kind: i64, from: i64, to: i64, next: i64 };

/// The line at `at`.
pub fn read(b: []u8, at: i64) Pkt {
    const n = array.len(b);
    if (at + 4 > n) { return broken(at); }
    var length = 0;
    var i = 0;
    while (i < 4) : (i += 1) {
        const d = hex_value(i64(b[at + i]));
        if (d < 0) { return broken(at); }
        length = length * 16 + d;
    }
    if (length == 0) { return Pkt{ .kind = FLUSH, .from = at + 4, .to = at + 4, .next = at + 4 }; }
    if (length == 1) { return Pkt{ .kind = DELIM, .from = at + 4, .to = at + 4, .next = at + 4 }; }
    if (length == 2) {
        return Pkt{ .kind = RESPONSE_END, .from = at + 4, .to = at + 4, .next = at + 4 };
    }
    // A length of three is a line shorter than its own header.
    if (length < 4 or at + length > n) { return broken(at); }
    return Pkt{ .kind = DATA, .from = at + 4, .to = at + length, .next = at + length };
}

fn broken(at: i64) Pkt {
    return Pkt{ .kind = BROKEN, .from = at, .to = at, .next = at };
}

fn hex_value(c: i64) i64 {
    if (c >= 48 and c <= 57) { return c - 48; }
    if (c >= 97 and c <= 102) { return c - 87; }
    if (c >= 65 and c <= 70) { return c - 55; }
    return -1;
}

/// A line's payload as text, without the newline it usually ends with.
pub fn trimmed(b: []u8, p: Pkt) str {
    var to = p.to;
    if (to > p.from and b[to - 1] == 10) { to -= 1; }
    return bytes.slice_str(b, p.from, to);
}

/// Write one line, newline included, which is what git expects of a command.
pub fn put(out: bytes.Buf, line: str) void {
    const payload = text.concat(line, "\n");
    put_raw(out, payload);
    return;
}

/// The same without adding anything.
pub fn put_raw(out: bytes.Buf, payload: str) void {
    bytes.put_str(out, hex4(text.len(payload) + 4));
    bytes.put_str(out, payload);
    return;
}

pub fn put_flush(out: bytes.Buf) void { bytes.put_str(out, "0000"); return; }
pub fn put_delim(out: bytes.Buf) void { bytes.put_str(out, "0001"); return; }

fn hex4(n: i64) str {
    const digits = "0123456789abcdef";
    var out = "";
    var shift = 12;
    while (shift >= 0) : (shift -= 4) {
        out = text.concat(out, text.from_byte(text.byte_at(digits, (n >> shift) & 15)));
    }
    return out;
}

// ---------------------------------------------------------------------------
// Side bands
// ---------------------------------------------------------------------------
//
// Once a fetch reaches its packfile, every line carries a band number in its
// first byte: 1 is the pack, 2 is progress meant for a terminal, and 3 is a
// fatal error the server wants read. Anything else is a protocol this client
// does not know.

pub const BAND_PACK = 1;
pub const BAND_PROGRESS = 2;
pub const BAND_ERROR = 3;

pub fn band(b: []u8, p: Pkt) i64 {
    if (p.kind != DATA or p.to <= p.from) { return -1; }
    return i64(b[p.from]);
}

/// A banded line's payload, which starts one byte after the band.
pub fn band_from(p: Pkt) i64 { return p.from + 1; }
