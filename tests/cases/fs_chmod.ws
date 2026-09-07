// Setting a file's permission bits, and seeing that they took.
//
// The pair exists for one caller: a program that writes another program. A file
// written by `io.write_file` is 0o644 and 0o644 cannot be run, so a tool that
// unpacks a compiler -- or writes a shim -- has to be able to say otherwise.
//
// `is_executable` rather than a mode reader is the whole design. Reading the
// bits back would mean a `struct stat`, which has four layouts across the
// systems `crate::sys` covers; `access(X_OK)` answers the question actually
// being asked, in one call, on all of them.
//
// **Windows has no permission bits**, and its arm succeeds without doing
// anything while `is_executable` falls back to `exists`. So the assertions here
// are the ones true on every platform: a file that is there is runnable after
// `chmod 0o755`, and a path that is not there is neither. "Not executable after
// 0o644" is deliberately *not* asserted -- it is false on Windows, and a case
// that passed only on Unix would be a case that fails the release.
//
// expect: chmod 0o755 raised: false
// expect: executable after 0o755: true
// expect: chmod 0o644 raised: false
// expect: missing file is not executable: true
// expect: chmod on a missing file raised: true
// expect: chmod of a directory raised: false
// expect: target is not empty: true
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fs = @import("std/fs");
const io = @import("std/io");
const os = @import("std/os");
const path = @import("std/path");
const text = @import("std/str");

fn main() i64 {
    run() catch {
        print("something raised");
        return 1;
    };
    return 0;
}

fn run() !void {
    // Named with random bytes because the suite runs a second time under
    // `--gc-stress` and the two runs may overlap, which is what
    // `tests/cases/os_process.ws` does for the same reason.
    const tag = bytes.to_hex(try crypto.random(8));
    const dir = path.join(os.temp_dir(), text.concat("wsharp-chmod-", tag));
    try fs.mkdir_all(dir);

    const file = path.join(dir, "program");
    try io.write_file(file, "#!/bin/sh\nexit 0\n");

    // The one that matters: a written file is 0o644, and this is what makes it
    // something the system will start.
    print(text.concat("chmod 0o755 raised: ", show(raised(file, 0o755))));
    print(text.concat("executable after 0o755: ", show(fs.is_executable(file))));

    // Back down again. Whether the bit actually cleared is platform-dependent,
    // so only the call is asserted -- see the note above.
    print(text.concat("chmod 0o644 raised: ", show(raised(file, 0o644))));

    // A path that is not there is not runnable, and saying so is an error
    // rather than a silent success.
    const missing = path.join(dir, "not-here");
    print(text.concat("missing file is not executable: ", show(!fs.is_executable(missing))));
    print(text.concat("chmod on a missing file raised: ", show(raised(missing, 0o755))));

    // A directory has permission bits like anything else, and 0o755 is what it
    // already is -- so this is a success, not a special case.
    print(text.concat("chmod of a directory raised: ", show(raised(dir, 0o755))));

    // Not a permissions question, but the other half of what a version manager
    // needs and too small for a case of its own. The value differs per machine,
    // so the shape is all that can be asserted.
    print(text.concat("target is not empty: ", show(text.len(os.target()) > 0)));

    try fs.remove_tree(dir);
    return;
}

/// Whether `chmod` raised, rather than what it raised.
///
/// The error set differs by platform for the same underlying condition, so a
/// case that named one would be a case about the platform it was written on.
fn raised(p: str, mode: i64) bool {
    fs.chmod(p, mode) catch { return true; };
    return false;
}

fn show(b: bool) str {
    if (b) { return "true"; }
    return "false";
}
