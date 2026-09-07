// The content-addressed store: the tree hash, an atomic install, and the
// difference between "not installed" and "damaged" -- which is the distinction
// the whole of `ingot verify` is built on, and the one a store that only asked
// whether a directory existed would not be able to make.
// expect: 0
// expect: same
// expect: different
// expect: 3
// expect: 1
// expect: same
// expect: 0
// expect: A
// expect: a
// expect: aa
// expect: b
// expect: 1
// expect: 0
// expect: 1
const array = @import("std/array");
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fault = @import("ingot/fault");
const fs = @import("std/fs");
const io = @import("std/io");
const list = @import("std/list");
const os = @import("std/os");
const path = @import("std/path");
const store = @import("ingot/store");
const text = @import("std/str");

fn run(f: fault.Fault, root: str) !i64 {
    const src = path.join(root, "pkg");
    const h = path.join(root, "home");
    try fs.mkdir_all(path.join(src, "src"));
    try io.write_file(path.join(src, "ingot.toml"), "[package]\nname = \"a\"\nversion = \"1.0.0\"\n");
    try io.write_file(path.join(src, "src/a.ws"), "pub fn hi() i64 { return 1; }\n");

    const digest = store.install(f, h, src);
    if (!f.ok) { print(f.message); return 1; }
    print_int(store.check(h, digest));

    // The digest is the contents, so the same tree is the same key and no work.
    print(same(store.install(f, h, src), digest));

    // One byte apart is a different key, which is what makes the store safe to
    // share between two projects that disagree about a version.
    try io.write_file(path.join(src, "src/a.ws"), "pub fn hi() i64 { return 2; }\n");
    const other = store.install(f, h, src);
    print(same(other, digest));

    // Editing an installed entry is `damaged`, not `ready`, and asking about
    // one that was never there is `missing`.
    try io.write_file(path.join(store.entry(h, digest), "src/a.ws"), "tampered\n");
    print_int(store.check(h, digest));
    print_int(store.check(h, "0000000000000000000000000000000000000000000000000000000000000000"));

    // And `install` repairs it rather than reporting success over it.
    try io.write_file(path.join(src, "src/a.ws"), "pub fn hi() i64 { return 1; }\n");
    print(same(store.install(f, h, src), digest));
    print_int(store.check(h, digest));

    // The order a filesystem lists a directory in is not sorted and is not the
    // same on two machines, so the hash sorts for itself. This is half of the
    // hash's definition rather than a convenience.
    for (store.sorted([]str{ "b", "a", "aa", "A" })) |n| { print(n); }

    // What no environment reaches goes.
    var keep: list.List[str] = list.new();
    list.push(keep, other);
    print_int(store.collect(f, h, keep));
    print_int(store.check(h, other));
    print_int(store.check(h, digest));
    return 0;
}

fn same(a: str, b: str) str {
    if (text.eq(a, b)) { return "same"; }
    return "different";
}

fn main() i64 {
    const f = fault.none();
    // Under the system's own temporary directory with a random name, because
    // this case runs twice -- once plainly and once under `--gc-stress`.
    const suffix = bytes.to_hex(crypto.random(6) catch return 2);
    const root = path.join(os.temp_dir(), text.concat("ingot-store-", suffix));
    const code = run(f, root) catch 1;
    if (!f.ok) { print(f.message); }
    fs.remove_tree(root) catch print("could not clean up");
    return code;
}
