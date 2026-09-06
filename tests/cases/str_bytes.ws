// Looking *into* a string, which is what every protocol above a socket needs.
// expect: 72
// expect: Hi
// expect: content-length
// expect: trimmed
// expect: 4096
// expect: -17
// expect: bad
const str = @import("std/str");

fn main() i64 {
    print_int(str.byte_at("Hi", 0));
    print(str.concat(str.from_byte(72), str.from_byte(105)));
    print(str.to_lower("Content-Length"));
    print(str.trim("  \t trimmed \r\n "));

    print_int(str.parse_int("4096") catch return 1);
    print_int(str.parse_int("-17") catch return 2);

    // Strict: a partial answer would be worse than no answer.
    const junk = str.parse_int("12x") catch |e| {
        if (e == error.BadFormat) { print("bad"); } else { print("wrong"); }
        return 0;
    };
    print_int(junk);
    return 3;
}
