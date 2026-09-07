// The process's own arguments and environment.
//
// `main` takes no arguments -- the type checker says so, and the one word a
// compiled `main` receives is the closure environment pointer every W#
// function takes. So the command line arrives out of band, published into the
// runtime before the program starts, and is read from here.
//
// `raw_args` answers with one blob of length-prefixed arguments rather than a
// `[]str`, because a builtin may not allocate an array: `array.new` is lowered
// inline, since only the call site knows the element type. Cutting the blob up
// is this file's whole job, and it is the same split `std/crypto` and
// `std/x509` already use for the system's root store.
const array = @import("std/array");
const bytes = @import("std/bytes");
const text = @import("std/str");

/// The arguments this program was given, without the name it was invoked by.
///
/// `wsharp run prog.ws -- a b c` gives `["a", "b", "c"]`, and so does
/// `ingot`'s own command line minus the verb's own reader.
pub fn args() []str {
    return unpack(raw_args());
}

/// The value of one environment variable, or null.
///
/// Null rather than an error, because a variable that is not set is the
/// ordinary case and `orelse` is what a caller wants to write.
pub fn get(name: str) ?str {
    return env(name);
}

/// The current user's home directory.
///
/// `HOME` on Unix and `USERPROFILE` on Windows, tried in that order rather
/// than chosen by platform: a Unix shell that sets `USERPROFILE` is nobody's
/// problem, and a Windows one that sets `HOME` -- which every Git installation
/// does -- is the answer the user meant.
///
/// An empty value counts as unset. A store rooted at `""` would be a store
/// rooted at the current directory, which is much worse than an error.
pub fn home() !str {
    if (env("HOME")) |h| {
        if (text.len(h) > 0) { return h; }
    }
    if (env("USERPROFILE")) |h| {
        if (text.len(h) > 0) { return h; }
    }
    return error.NotFound;
}

/// The directory this process is running in.
///
/// Fallible where `home` and `temp_dir` are not: those ask the environment,
/// which either says something or does not, while this asks the kernel about a
/// directory that can have been removed since the process entered it.
///
/// The separator is whatever the system wrote -- a Windows answer holds `\`.
/// `path.normalise` is what turns one into a `/`, and it is the caller's to
/// call: `std/path` imports nothing, and this module importing it would be the
/// wrong way round for the reason `trimmed` below is written out by hand.
pub fn cwd() !str {
    return raw_cwd();
}

/// Where this system keeps files nobody intends to keep.
///
/// `TMPDIR` is what macOS and the BSDs set, `TMP` and `TEMP` are what Windows
/// sets, and a Linux that sets none of the three means `/tmp`. Asked in that
/// order rather than chosen by platform, for `home`'s reason: an environment
/// that says where it wants temporary files is telling the truth about itself,
/// whatever system it is running on.
///
/// There is no trailing separator, so `path.join` produces the same spelling
/// whichever branch answered.
pub fn temp_dir() str {
    if (env("TMPDIR")) |d| {
        if (text.len(d) > 0) { return trimmed(d); }
    }
    if (env("TMP")) |d| {
        if (text.len(d) > 0) { return trimmed(d); }
    }
    if (env("TEMP")) |d| {
        if (text.len(d) > 0) { return trimmed(d); }
    }
    return "/tmp";
}

/// A directory without its trailing separator, so that joining is unambiguous.
///
/// Not `path.trim_end`, because `std/path` imports nothing and this module
/// importing it would be the wrong way round: a path is arithmetic and the
/// environment is a fact about the process.
fn trimmed(d: str) str {
    var n = text.len(d);
    while (n > 1) : (n -= 1) {
        const b = text.byte_at(d, n - 1);
        if (b != 47 and b != 92) { break; }
    }
    return text.substr(d, 0, n);
}

/// A blob of four-byte big-endian lengths and their bytes, cut back up.
///
/// Shared with `std/fs.read_dir`, which is handed the same shape. A length the
/// blob cannot hold ends the walk rather than raising: the blob comes from the
/// runtime beside this file, so a malformed one is a bug here and not a
/// condition a caller could do anything about.
pub fn unpack(blob: str) []str {
    const b = bytes.of(blob);
    const n = array.len(b);
    var out = []str{};
    var at = 0;
    while (at + 4 <= n) {
        const size = i64(bytes.be32(b, at));
        at += 4;
        if (size < 0 or at + size > n) { return out; }
        out = array.push(out, bytes.slice_str(b, at, at + size));
        at += size;
    }
    return out;
}
