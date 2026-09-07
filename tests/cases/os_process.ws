// The two things a program needs to be a command line tool rather than a
// calculation: knowing where it is, and being able to work somewhere else.
//
// `exec` is not here, because it does not come back -- a case that called it
// could assert nothing afterwards, and the thing that exercises it is `ingot
// run`, which is exactly what it was added for.
//
// expect: self_exe is absolute: true
// expect: chdir moved us: true
// expect: chdir back: true
// expect: chdir to nowhere failed
// expect: pack round-trips: true
const fs = @import("std/fs");
const os = @import("std/os");
const path = @import("std/path");
const text = @import("std/str");
const array = @import("std/array");
const crypto = @import("std/crypto");
const bytes = @import("std/bytes");

fn main() i64 {
    run() catch {
        print("something raised");
        return 1;
    };
    return 0;
}

fn run() !void {
    // The running executable. Under the case suite that is `wsharp` itself and
    // under the built pass it is this case's own binary, so the only thing
    // worth asserting is the shape the two share.
    const me = try os.self_exe();
    print(text.concat("self_exe is absolute: ", show(path.is_absolute(path.normalise(me)))));

    // A directory of our own, named with random bytes because the suite runs a
    // second time under stress and the two runs may overlap.
    const tag = bytes.to_hex(try crypto.random(8));
    const dir = path.join(os.temp_dir(), text.concat("wsharp-os-", tag));
    try fs.mkdir_all(dir);

    const before = try os.cwd();
    try os.chdir(dir);
    // Compared by what it contains rather than for equality: macOS answers
    // `/private/var/...` for a `/var/...` it was given, and both are true.
    const now = try os.cwd();
    print(text.concat("chdir moved us: ", show(text.find(now, tag) >= 0)));

    try os.chdir(before);
    print(text.concat("chdir back: ", show(text.eq(try os.cwd(), before))));

    // A directory that is not there is an error rather than a silent no-op,
    // which is the whole reason this is `!void`.
    os.chdir(path.join(dir, "not-here")) catch print("chdir to nowhere failed");

    // What `exec` hands the runtime. Checked here because `exec` itself cannot
    // be: the packing is the half that can go wrong quietly, and `unpack` is
    // the same framing read back.
    const args = []str{ "run", "", "a b", tag };
    const back = os.unpack(os.pack(args));
    print(text.concat("pack round-trips: ", show(same(args, back))));

    try fs.remove_tree(dir);
    return;
}

fn same(a: []str, b: []str) bool {
    const n = array.len(a);
    if (n != array.len(b)) { return false; }
    var i = 0;
    while (i < n) : (i += 1) {
        if (!text.eq(a[i], b[i])) { return false; }
    }
    return true;
}

fn show(b: bool) str {
    if (b) { return "true"; }
    return "false";
}
