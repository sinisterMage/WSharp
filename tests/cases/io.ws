// File I/O, which is where the standard library first has to *fail*. A
// fallible builtin returns a `!T`: a tag and a payload, crossing the boundary
// as the two words the C ABI returns a pair in.
//
// The file goes under the system's own temporary directory with a random name,
// which is the rule every other case that touches the filesystem follows and
// this one used to be the exception to. Two reasons, and the second is the one
// that bites: `/tmp` is not a place on Windows -- it resolves to the current
// drive's root, which need not exist -- and the suite runs this file more than
// once at a time, so a fixed name is two processes opening one file, which
// Windows refuses rather than tolerates.
// expect: 18
// expect: line one
// expect: line two
// expect:
// expect: absent
// expect: true
// expect: false
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fs = @import("std/fs");
const io = @import("std/io");
const os = @import("std/os");
const path = @import("std/path");
const str = @import("std/str");

fn save(where: str, text: str) !void { return io.write_file(where, text); }

fn main() i64 {
    const suffix = bytes.to_hex(crypto.random(6) catch return 2);
    const here = path.join(os.temp_dir(), str.concat("wsharp-io-", suffix));
    // Never written, so there is nothing to clean up after it.
    const absent = str.concat(here, "-absent");

    save(here, "line one\nline two\n") catch print("could not write");

    const text = io.read_file(here) catch "";
    print_int(str.len(text));
    for (str.split(text, "\n")) |line| { print(line); }

    // A missing file is an ordinary error, caught like any other.
    print(io.read_file(absent) catch "absent");
    print_bool(io.exists(here));
    print_bool(io.exists(absent));

    fs.remove(here) catch print("could not clean up");
    return 0;
}
