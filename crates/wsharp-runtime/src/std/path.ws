// Paths, as arithmetic on strings.
//
// Nothing here touches the filesystem, and that is the point: a store spends
// most of its time deciding *where* a thing goes and comparatively little
// putting it there, and a question that can be answered without a syscall
// should be.
//
// **The separator is `/` everywhere, including Windows.** Every Win32 path
// call accepts one, `sys/windows.rs` hands the wildcard for a directory listing
// through unchanged, and one spelling is what keeps a lockfile written on one
// machine readable on another. A `\` that arrives from outside is normalised
// away; none is ever produced.
const text = @import("std/str");
const array = @import("std/array");

const SLASH = 47;     // '/'
const BACKSLASH = 92; // '\'
const DOT = 46;       // '.'
const COLON = 58;     // ':'

/// Whether `p` names a place rather than a place relative to somewhere else.
///
/// Two spellings, because there are two kinds of absolute path in the world:
/// a leading `/`, and a Windows drive letter. `C:foo` is neither -- it is
/// relative to the current directory *of that drive*, which is a concept this
/// library declines to have.
pub fn is_absolute(p: str) bool {
    const n = text.len(p);
    if (n == 0) { return false; }
    if (text.byte_at(p, 0) == SLASH or text.byte_at(p, 0) == BACKSLASH) { return true; }
    if (n >= 3 and text.byte_at(p, 1) == COLON) {
        const sep = text.byte_at(p, 2);
        return sep == SLASH or sep == BACKSLASH;
    }
    return false;
}

/// `b` reached from `a`.
///
/// An absolute `b` wins outright, which is what every path library does and
/// what makes `join(root, from_a_config_file)` safe to write.
pub fn join(a: str, b: str) str {
    if (text.len(b) == 0) { return a; }
    if (text.len(a) == 0 or is_absolute(b)) { return b; }
    return text.concat(text.concat(trim_end(a), "/"), b);
}

/// Everything before the last separator, or `"."` when there is none.
///
/// `"/"` is its own parent, as it is everywhere: a root has nowhere above it
/// and answering `""` would turn a walk upwards into an infinite one.
pub fn dirname(p: str) str {
    const at = last_sep(p);
    if (at < 0) { return "."; }
    if (at == 0) { return "/"; }
    return text.substr(p, 0, at);
}

/// Everything after the last separator.
pub fn basename(p: str) str {
    const at = last_sep(p);
    if (at < 0) { return p; }
    return text.substr(p, at + 1, text.len(p));
}

/// The final `.suffix` of the last component, including the dot, or `""`.
///
/// A leading dot is a hidden file rather than an extension, so `.gitignore`
/// has none -- which is the rule every other library settled on and the one
/// that stops `dirname`/`basename`/`extension` from disagreeing about
/// `.ingot`.
pub fn extension(p: str) str {
    const name = basename(p);
    var i = text.len(name) - 1;
    while (i > 0) : (i -= 1) {
        if (text.byte_at(name, i) == DOT) { return text.substr(name, i, text.len(name)); }
    }
    return "";
}

/// The same place, spelled canonically.
///
/// Separators become `/`, runs of them collapse, `.` is dropped and `..`
/// cancels the component before it. Purely textual: no symbolic link is
/// followed and nothing is looked up, so this answers for a path that does not
/// exist and answers the same on every machine -- which is what a lockfile
/// needs and what `canonicalize` cannot give.
///
/// A `..` that walks off the front of a relative path is kept, because
/// `../sibling` means something; one that walks off the front of an absolute
/// path is dropped, because `/..` is `/`.
pub fn normalise(p: str) str {
    const absolute = is_absolute(p);
    // The drive letter is taken off first: `C:` would otherwise split into a
    // component of its own and be written twice.
    const head = drive(p);
    const rest = text.substr(p, text.len(head), text.len(p));
    var out = []str{};
    for (text.split(replace_backslashes(rest), "/")) |part| {
        if (text.len(part) == 0 or text.eq(part, ".")) { continue; }
        if (text.eq(part, "..")) {
            const n = array.len(out);
            if (n > 0 and !text.eq(out[n - 1], "..")) {
                out = array.slice(out, 0, n - 1);
                continue;
            }
            if (absolute) { continue; }
        }
        out = array.push(out, part);
    }
    const body = text.join(out, "/");
    if (absolute) {
        // A drive letter keeps its own head: `C:/a/../b` is `C:/b`, not `/b`.
        if (text.len(head) > 0) { return text.concat(text.concat(head, "/"), body); }
        return text.concat("/", body);
    }
    if (text.len(body) == 0) { return "."; }
    return body;
}

/// `C:` when `p` starts with a drive letter, and `""` otherwise.
///
/// Public because a walk *down* a path has to start somewhere, and on Windows
/// that somewhere is the drive rather than `/`. `std/fs.mkdir_all` seeds itself
/// with this; building `/C:/Users` instead is a `mkdir` that fails on its very
/// first step.
pub fn drive(p: str) str {
    if (text.len(p) >= 2 and text.byte_at(p, 1) == COLON) { return text.substr(p, 0, 2); }
    return "";
}

fn replace_backslashes(p: str) str {
    if (text.find(p, "\\") < 0) { return p; }
    var out = "";
    var i = 0;
    const n = text.len(p);
    while (i < n) : (i += 1) {
        const b = text.byte_at(p, i);
        if (b == BACKSLASH) { out = text.concat(out, "/"); } else { out = text.concat(out, text.from_byte(b)); }
    }
    return out;
}

/// The index of the last separator, or -1.
fn last_sep(p: str) i64 {
    var i = text.len(p) - 1;
    while (i >= 0) : (i -= 1) {
        const b = text.byte_at(p, i);
        if (b == SLASH or b == BACKSLASH) { return i; }
    }
    return -1;
}

/// `a` without its trailing separators, except that `"/"` keeps its own.
fn trim_end(a: str) str {
    var n = text.len(a);
    while (n > 1) : (n -= 1) {
        const b = text.byte_at(a, n - 1);
        if (b != SLASH and b != BACKSLASH) { break; }
    }
    return text.substr(a, 0, n);
}
