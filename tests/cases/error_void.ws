// `!void`: a function that can fail and has nothing to report when it does not.
//
// `return;` there means "finished, and nothing went wrong" -- the union is the
// success tag with no payload beside it. Until this worked, `!void` was a type
// a builtin could return and no W# function could be written to produce.
// expect: 0
// expect: -1
// expect: -2
fn checked(n: i64) !{Negative, Zero}void {
    if (n < 0) { return error.Negative; }
    if (n == 0) { return error.Zero; }
    return;
}

fn tagged(n: i64) i64 {
    checked(n) catch |e| {
        if (e == error.Negative) { return -1; }
        return -2;
    };
    return 0;
}

fn main() i64 {
    print_int(tagged(3));
    print_int(tagged(-5));
    print_int(tagged(0));
    return 0;
}
