// `|` is both the bitwise operator and capture syntax, and the two never meet:
// a capture is only ever looked for straight after `catch`, or after the `)`
// closing a `while`, `for` or `if` header. Everywhere else a `|` is an
// operator. This is the case that keeps it that way.
// expect: 6
// expect: 2
// expect: 12
// expect: 5
fn fallible(n: i64) !i64 { if (n < 0) { return error.Negative; } return n; }

fn main() i64 {
    // Inside a condition, where the header's `)` has not been reached.
    // `|` binds tighter than `!=`, unlike C, so this is `(i | 1) != 7`.
    var i = 0;
    while (i | 1 != 7) : (i += 2) { }
    print_int(i);

    // After a `catch`, where the next token really could have been a capture.
    // `|` binds tighter than `catch`, so the alternative is `0 | 2`.
    print_int(fallible(-1) catch 0 | 2);

    // And with a capture immediately following one.
    const got = fallible(-1) catch |e| if (e == error.Negative) 8 | 4 else 0;
    print_int(got);

    // An optional capture on a `while`, with an operator in the condition.
    var seen = 0;
    const maybe: ?i64 = 5;
    while (maybe) |v| { seen = v | 1; break; }
    print_int(seen);
    return 0;
}
