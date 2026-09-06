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
pub fn repeat(s: str, n: i64) str {
    var out = "";
    var i = 0;
    while (i < n) : (i += 1) { out = concat(out, s); }
    return out;
}

/// Whether `s` starts with `prefix`.
pub fn starts_with(s: str, prefix: str) bool {
    if (len(prefix) > len(s)) { return false; }
    return eq(substr(s, 0, len(prefix)), prefix);
}

/// The pieces of `parts` with `sep` between them.
pub fn join(parts: []str, sep: str) str {
    var out = "";
    var first = true;
    for (parts) |p| {
        if (first) { first = false; } else { out = concat(out, sep); }
        out = concat(out, p);
    }
    return out;
}
