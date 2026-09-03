// expect: 0
// expect: 1
// expect: 1
// expect: 2
// expect: 3
// expect: 55
fn fib(n: i64) i64 {
    if (n < 2) { return n; }
    return fib(n - 1) + fib(n - 2);
}
fn main() i64 {
    var i: i64 = 0;
    while (i < 5) : (i += 1) { print_int(fib(i)); }
    print_int(fib(10));
    return 0;
}
