// Paths, as arithmetic on strings: no syscall is made here and none is needed.
// The separator is `/` on every platform, and a `\` that arrives from outside
// is normalised away rather than produced.
// expect: a/b
// expect: a/b
// expect: /b
// expect: b
// expect: /a/b
// expect: .
// expect: /
// expect: c.ws
// expect: c.ws
// expect: .ws
// expect:
// expect:
// expect: /a/c/d
// expect: ../b
// expect: .
// expect: /
// expect: C:/a/c
// expect: true
// expect: true
// expect: false
// expect: false
const path = @import("std/path");

fn main() i64 {
    print(path.join("a", "b"));
    print(path.join("a///", "b"));
    // An absolute right-hand side wins outright, which is what makes joining a
    // configured path onto a root safe to write.
    print(path.join("a", "/b"));
    print(path.join("", "b"));

    print(path.dirname("/a/b/c.ws"));
    print(path.dirname("c.ws"));
    print(path.dirname("/c.ws"));
    print(path.basename("/a/b/c.ws"));
    print(path.basename("c.ws"));

    print(path.extension("/a/b/c.ws"));
    print(path.extension("/a/b/c"));
    // A leading dot is a hidden file, not an extension.
    print(path.extension("/a/.gitignore"));

    print(path.normalise("/a/b/../c//./d"));
    // `..` off the front of a relative path is kept, because `../sibling`
    // means something; off the front of an absolute one it is dropped.
    print(path.normalise("a/../../b"));
    print(path.normalise("a/.."));
    print(path.normalise("/../.."));
    print(path.normalise("C:\\a\\b\\..\\c"));

    print_bool(path.is_absolute("/x"));
    print_bool(path.is_absolute("C:/x"));
    print_bool(path.is_absolute("x"));
    // A drive-relative path is neither, and this library declines to have the
    // concept.
    print_bool(path.is_absolute("C:x"));
    return 0;
}
