// expect: 8
// expect: 0
// expect: 9
// expect: -7
fn risky(n: i64) !i64 {
    if (n < 0) { return error.Negative; }
    return n * 2;
}
fn chain(n: i64) !i64 {
    const v = try risky(n);
    return v + 1;
}
fn main() i64 {
    print_int(risky(4) catch 0);
    print_int(risky(-1) catch 0);
    print_int(chain(4) catch 0);
    print_int(chain(-1) catch |e| -7);
    return 0;
}
