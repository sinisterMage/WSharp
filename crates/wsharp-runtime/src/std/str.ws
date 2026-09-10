// String operations that build arrays.
//
// The rest of `std/str` is in the builtin table: a string holds no references,
// so a runtime function may assemble one freely. `split` returns `[]str`,
// which does hold them, so it is written here -- see `std/array.ws` for why.
const array = @import("std/array");

/// The pieces of `s` between each occurrence of `sep`.
///
/// An empty separator yields the whole string, rather than every character:
/// splitting on nothing is much more often a bug than a request.
pub fn split(s: str, sep: str) []str {
    if (len(sep) == 0) { return []str{ s }; }

    var out = []str{};
    var rest = s;
    var going = true;
    while (going) {
        const at = find(rest, sep);
        if (at < 0) {
            going = false;
        } else {
            out = array.push(out, substr(rest, 0, at));
            rest = substr(rest, at + len(sep), len(rest));
        }
    }
    return array.push(out, rest);
}

/// `s` repeated `n` times.
///
/// Measured, allocated once, copied -- see [`join`] for why that is written out
/// rather than being `concat` in a loop.
pub fn repeat(s: str, n: i64) str {
    if (n <= 0) { return ""; }
    const width = len(s);
    const out: []u8 = array.new(width * n);
    var at = 0;
    var i = 0;
    while (i < n) : (i += 1) {
        raw_into(out, at, s);
        at += width;
    }
    return raw_from(out, 0, at);
}

/// Whether `s` starts with `prefix`.
pub fn starts_with(s: str, prefix: str) bool {
    if (len(prefix) > len(s)) { return false; }
    return eq(substr(s, 0, len(prefix)), prefix);
}

/// The pieces of `parts` with `sep` between them.
///
/// Two passes -- measure, allocate once, copy -- rather than `concat` in a
/// loop, which is what this was and which is quadratic: `concat` copies its
/// accumulator, so joining n pieces copies the answer n times. Joining 150,000
/// pieces into 600 KB took 9.4 seconds that way and is now linear. It is the
/// same shape `bytes.to_hex` is written in, and for the same reason.
///
/// `array.len` is read into `count` and `len(sep)` into `width` because a
/// builtin call is a stack walk under `--gc-stress`, and neither answer changes
/// while the loop runs.
pub fn join(parts: []str, sep: str) str {
    const count = array.len(parts);
    if (count == 0) { return ""; }
    const width = len(sep);

    var total = width * (count - 1);
    var i = 0;
    while (i < count) : (i += 1) { total += len(parts[i]); }

    const out: []u8 = array.new(total);
    var at = 0;
    i = 0;
    while (i < count) : (i += 1) {
        if (i > 0) {
            raw_into(out, at, sep);
            at += width;
        }
        const p = parts[i];
        raw_into(out, at, p);
        at += len(p);
    }
    return raw_from(out, 0, total);
}
