// One failure, with something to read on it.
//
// A W# error union carries a tag and nothing else, which is right for
// `NotFound` and useless for "line 14 of ingot.toml says `verison`". A package
// manager is judged on the day it goes wrong, so everything below the verbs
// takes a `Fault` and writes into it, exactly as `std/toml`'s reader writes
// into its own state: the first failure wins, later ones are noise from a
// reader that has already lost its place, and the verb at the top is the only
// thing that prints.
const text = @import("std/str");

pub const Fault = struct { ok: bool, message: str, report: fn(str) void };

fn silent(message: str) void { return; }

pub fn none() Fault { return reporting(silent); }

/// Libraries stay silent unless their caller supplies a progress sink.
pub fn reporting(report: fn(str) void) Fault {
    return Fault{ .ok = true, .message = "", .report = report };
}

pub fn status(f: Fault, message: str) void {
    if (f.ok) { f.report(message); }
    return;
}

/// End a transient line before stdout or a diagnostic writes a complete line.
/// An empty report is the sink's flush signal; a silent sink ignores it.
pub fn flush(f: Fault) void { f.report(""); return; }

/// Record a failure. The first one is kept; the rest are dropped.
pub fn fail(f: Fault, message: str) void {
    if (f.ok) {
        f.ok = false;
        f.message = message;
    }
    return;
}

/// The same, with a subject: `fail_at(f, "ingot.toml", "no `name`")`.
pub fn fail_at(f: Fault, where: str, message: str) void {
    fail(f, text.concat(where, text.concat(": ", message)));
    return;
}
