// Recursive Fibonacci: the smallest program that needs recursion,
// conditionals, and a loop all working together.

fn fib(n: i64) i64 {
    if (n < 2) { return n; }
    return fib(n - 1) + fib(n - 2);
}

fn main() i64 {
    var i: i64 = 0;
    while (i < 10) : (i += 1) {
        print_int(fib(i));
    }
    return 0;
}
