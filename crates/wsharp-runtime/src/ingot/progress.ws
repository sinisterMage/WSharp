// Human-readable status on stderr; stdout remains the verbs' TSV interface.
const io = @import("std/io");
const text = @import("std/str");

pub const Progress = struct { quiet: bool, active: bool };

pub fn new(quiet: bool) Progress {
    return Progress{ .quiet = quiet, .active = false };
}

pub fn status(p: Progress, message: str) void {
    if (!p.quiet) {
        if (text.len(message) == 0) {
            if (p.active) { io.progress("", true); }
            p.active = false;
            return;
        }
        io.progress(text.concat("ingot: ", message), false);
        p.active = true;
    }
    return;
}

pub fn finish(p: Progress, ok: bool) void {
    // An empty finish preserves the final count on a terminal and adds no
    // duplicate summary to redirected logs.
    if (p.active) {
        io.progress(if (ok) "" else "ingot: Failed.", true);
        p.active = false;
    }
    return;
}

/// Counts completed packages, including verified cache hits.
pub fn package(done: i64, total: i64, action: str, name: str) str {
    return text.join([]str{ "[", text.from_int(done), "/", text.from_int(total),
        "] ", action, " ", name }, "");
}
