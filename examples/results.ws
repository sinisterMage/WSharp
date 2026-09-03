// Optionals and error unions.
//
// `?T` is a value that may be absent; `!T` is one that may have failed. Both
// compile to a tag next to the payload -- no allocation, no boxing.
//
// A plain value coerces into either when the context asks for it, which is what
// lets `return n;` sit in a function declared `!i64`.

fn half(n: i64) ?i64 {
    if (n % 2 != 0) { return null; }
    return n / 2;
}

fn checked_div(a: i64, b: i64) !i64 {
    if (b == 0) { return error.DivideByZero; }
    return a / b;
}

// `try` unwraps an error union or returns the error from this function, so
// this one has to be fallible too.
fn average(a: i64, b: i64, count: i64) !i64 {
    const total = a + b;
    const mean = try checked_div(total, count);
    return mean;
}

fn main() i64 {
    // `orelse` supplies a value for the null case.
    print_int(half(10) orelse -1);
    print_int(half(7) orelse -1);

    // `|v|` binds the payload when it is present.
    if (half(8)) |v| { print_int(v); } else { print("odd"); }
    if (half(9)) |v| { print_int(v); } else { print("odd"); }

    // `.?` asserts presence, and aborts if it is wrong.
    print_int(half(6).?);

    // `catch` supplies a value for the error case, optionally binding the error.
    print_int(checked_div(10, 2) catch 0);
    print_int(checked_div(10, 0) catch 0);
    print_int(checked_div(10, 0) catch |e| -1);

    // The error propagated by `try` surfaces here.
    print_int(average(3, 7, 2) catch 0);
    print_int(average(3, 7, 0) catch -99);
    return 0;
}
