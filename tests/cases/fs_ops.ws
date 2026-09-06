// The rest of a filesystem: directories, a listing, `rename`, and the two
// facts about a path a content-addressed store needs. `std/io` is about a
// file's contents; this is about the tree it sits in.
//
// The directory is made under the system's own temporary directory with a
// random name, because this case runs twice -- once plainly and once under
// `--gc-stress` -- and the two runs may overlap.
// expect: true
// expect: true
// expect: 5
// expect: false
// expect: 3
// expect: true
// expect: true
// expect: true
// expect: already
// expect: not empty
// expect: false
// expect: hello
// expect: false
const array = @import("std/array");
const text = @import("std/str");
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fs = @import("std/fs");
const io = @import("std/io");
const os = @import("std/os");
const path = @import("std/path");

fn run(root: str) !i64 {
    try fs.mkdir_all(path.join(root, "a/b"));
    print_bool(fs.is_dir(root));
    print_bool(fs.is_dir(path.join(root, "a/b")));

    const inner = path.join(root, "a");
    try io.write_file(path.join(inner, "one.txt"), "hello");
    try io.write_file(path.join(inner, "two.txt"), "worldly");
    print_int(try fs.size(path.join(inner, "one.txt")));
    print_bool(fs.is_dir(path.join(inner, "one.txt")));

    // The order a directory lists in is the filesystem's and is not the same
    // on two machines, so the case counts and asks after each name rather than
    // comparing a joined string.
    const names = try fs.read_dir(inner);
    print_int(array.len(names));
    print_bool(holds(names, "b"));
    print_bool(holds(names, "one.txt"));
    print_bool(holds(names, "two.txt"));

    // A directory that is already there, and one that still holds something:
    // the two failures a store meets on its ordinary path.
    fs.mkdir(inner) catch |e| print(if (e == error.AlreadyExists) "already" else "wrong error");
    fs.rmdir(inner) catch |e| print(if (e == error.DirectoryNotEmpty) "not empty" else "wrong error");

    // `rename` is what makes an install atomic, so it is what this case is
    // really here for.
    try fs.rename(path.join(inner, "one.txt"), path.join(inner, "three.txt"));
    print_bool(io.exists(path.join(inner, "one.txt")));
    print(try io.read_file(path.join(inner, "three.txt")));

    try fs.remove_tree(root);
    print_bool(io.exists(root));
    return 0;
}

fn holds(names: []str, want: str) bool {
    for (names) |n| { if (n == want) { return true; } }
    return false;
}

fn main() i64 {
    const suffix = bytes.to_hex(crypto.random(6) catch return 2);
    const root = path.join(os.temp_dir(), text.concat("wsharp-fs-", suffix));
    const code = run(root) catch 1;
    // Whatever went wrong, the directory does not outlive the case.
    fs.remove_tree(root) catch print("could not clean up");
    return code;
}
