// The bridge stage four adds: a packfile becomes a tree in the store.
//
// Everything here is the recorded conversation from `git_protocol.ws` -- no
// socket is opened. What is being checked is the join: that what comes out of a
// real packfile goes into the store as files, that the store gives it a digest,
// and that fetching the same revision twice is remembered rather than done
// again.
// expect: 0
// expect: true
// expect: 42
// expect: 10085
// expect: same
// expect: not yet
// expect: remembered
const array = @import("std/array");
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fault = @import("ingot/fault");
const fixture = @import("./modules/gitfixture.ws");
const fs = @import("std/fs");
const git = @import("ingot/git");
const io = @import("std/io");
const list = @import("std/list");
const os = @import("std/os");
const path = @import("std/path");
const store = @import("ingot/store");
const text = @import("std/str");

fn run(f: fault.Fault, home: str) i64 {
    const body = bytes.to_str(bytes.from_hex(fixture.fetched()) catch return 2);
    const pack = git.parse_packfile(f, body) orelse { print(f.message); return 1; };
    const files = git.files(f, pack, fixture.HEAD) orelse { print(f.message); return 1; };

    var carried: list.List[store.File] = list.new();
    for (list.to_array(files)) |file| {
        list.push(carried, store.File{ .path = file.path, .data = file.data });
    }
    const digest = store.install_files(f, home, carried);
    if (!f.ok) { print(f.message); return 1; }
    print_int(store.check(home, digest));

    // The files are where the tree said, with the contents it said.
    const entry = store.entry(home, digest);
    print_bool(fs.is_dir(path.join(entry, "src")));
    print_int(fs.size(path.join(entry, "ingot.toml")) catch -1);
    print_int(fs.size(path.join(entry, "src/demo.ws")) catch -1);

    // Installing the same tree again is the same key: the digest is the
    // contents, so a second fetch of one revision cannot produce a second
    // entry.
    const again = store.install_files(f, home, carried);
    print(if (text.eq(again, digest)) "same" else "DIFFERENT");

    // And a revision names one tree for ever, so it is remembered rather than
    // fetched twice.
    const source = text.concat("git+https://example.invalid/demo.git#", fixture.HEAD);
    print(if (store.remembered(home, source)) |d| "REMEMBERED ALREADY" else "not yet");
    store.remember(f, home, source, digest);
    if (!f.ok) { print(f.message); return 1; }
    const back = store.remembered(home, source) orelse { print("FORGOTTEN"); return 1; };
    print(if (text.eq(back, digest)) "remembered" else "REMEMBERED WRONGLY");
    return 0;
}

fn main() i64 {
    const f = fault.none();
    const suffix = bytes.to_hex(crypto.random(6) catch return 2);
    const home = path.join(os.temp_dir(), text.concat("ingot-fetch-", suffix));
    const code = run(f, home);
    fs.remove_tree(home) catch print("could not clean up");
    return code;
}
