// The string half of the standard library, and `==` on `str` -- which used to
// be rejected because it needs a runtime call rather than an instruction.
// expect: 5
// expect: foobar
// expect: true
// expect: false
// expect: true
// expect: world
// expect: 6
// expect: -1
// expect: -42
// expect: 2.5
// expect: true
// expect: false
// expect: true
// expect: ab-ab-ab
// expect: true
const str = @import("std/str");
fn main() i64 {
    print_int(str.len("hello"));
    print(str.concat("foo", "bar"));
    print_bool(str.eq("a", "a"));
    print_bool(str.eq("a", "b"));
    print_bool(str.starts_with("hello", "hell"));
    print(str.substr("hello world", 6, 11));
    print_int(str.find("hello world", "world"));
    print_int(str.find("hello", "zzz"));
    print(str.from_int(-42));
    print(str.from_float(2.5));

    // `==` compares contents, so a heap-built string equals a literal.
    print_bool(str.concat("ab", "c") == "abc");
    print_bool(str.concat("ab", "c") != "abc");
    print_bool("x" != "y");

    print(str.join([]str{ "ab", "ab", "ab" }, "-"));
    print_bool(str.repeat("ab", 3) == "ababab");
    return 0;
}
