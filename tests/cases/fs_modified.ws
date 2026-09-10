// expect: a file just written is not from the past
// expect: a file just written is not from the future
// expect: rewriting a file does not make it older
// expect: a directory has a time too
// expect: a path that is not there is NotFound
// `fs.modified_at`: when a path was last written.
//
// The one question `std/fs` asks that does not have a one-number answer on
// every system -- Linux has `statx`, macOS has `getattrlist`, Windows was
// already fetching the field and throwing it away, and the rest of the BSDs
// mean `stat(2)` and one offset per system. It is asked anyway because a
// watcher that had to hash a whole tree on every tick is a real cost.
//
// What is checked is what can be: the answer is on `time.now()`'s clock and
// within a few seconds of it. A tighter bound would be a test that fails on a
// slow machine, and a looser one would pass with the wrong field read -- an
// inode number or a device number at the wrong offset is not a plausible
// timestamp.
//
// A directory of its own with random bytes in the name, removed at the end,
// because the suite runs a second time under `--gc-stress` and the two runs may
// overlap.
const fs = @import("std/fs");
const io = @import("std/io");
const os = @import("std/os");
const path = @import("std/path");
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const time = @import("std/time");
const text = @import("std/str");

/// A window wide enough for a slow machine and narrow enough that a wrong
/// field is outside it.
const SLACK: i64 = 120;

fn main() i64 {
    const dir = path.join(os.temp_dir(),
        text.concat("wsharp-mtime-", bytes.to_hex(crypto.random(8) catch return 1)));
    fs.mkdir_all(dir) catch return 2;

    const file = path.join(dir, "note.txt");
    io.write_file(file, "one") catch return 3;

    const now = time.now();
    const written = fs.modified_at(file) catch return 4;
    if (written > now - SLACK) { print("a file just written is not from the past"); }
    if (written < now + SLACK) { print("a file just written is not from the future"); }

    io.write_file(file, "one and a bit more") catch return 5;
    const again = fs.modified_at(file) catch return 6;
    if (again >= written) { print("rewriting a file does not make it older"); }

    const on_dir = fs.modified_at(dir) catch return 7;
    if (on_dir > now - SLACK and on_dir < now + SLACK) { print("a directory has a time too"); }

    fs.modified_at(path.join(dir, "absent.txt")) catch |e| {
        if (e == error.NotFound) { print("a path that is not there is NotFound"); }
        fs.remove_tree(dir) catch return 8;
        return 0;
    };
    fs.remove_tree(dir) catch return 9;
    return 10;
}
