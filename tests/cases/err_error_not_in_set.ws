// error: `error.Zero` is not one of `{Negative}`
// error: which this function cannot
fn wrong(n: i64) !{Negative}i64 {
    if (n == 0) { return error.Zero; }
    return n;
}
fn raises(n: i64) !i64 {
    if (n < 0) { return error.Other; }
    return n;
}
fn narrow(n: i64) !{Negative}i64 { return try raises(n); }
