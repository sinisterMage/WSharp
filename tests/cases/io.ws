// File I/O, which is where the standard library first has to *fail*. A
// fallible builtin returns a `!T`: a tag and a payload, crossing the boundary
// as the two words the C ABI returns a pair in.
// expect: 18
// expect: line one
// expect: line two
// expect:
// expect: absent
// expect: true
// expect: false
// The file is written before it is read, so the case needs no fixture.
const io = @import("std/io");
const str = @import("std/str");

fn save(path: str, text: str) !void { return io.write_file(path, text); }

fn main() i64 {
    const path = "/tmp/wsharp_case_io.txt";
    save(path, "line one\nline two\n") catch print("could not write");

    const text = io.read_file(path) catch "";
    print_int(str.len(text));
    for (str.split(text, "\n")) |line| { print(line); }

    // A missing file is an ordinary error, caught like any other.
    print(io.read_file("/tmp/wsharp_case_definitely_absent") catch "absent");
    print_bool(io.exists(path));
    print_bool(io.exists("/tmp/wsharp_case_definitely_absent"));
    return 0;
}
