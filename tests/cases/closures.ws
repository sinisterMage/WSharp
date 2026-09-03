// expect: 3
// expect: 105
// expect: 7
fn twice(f: fn(i64) i64, x: i64) i64 { return f(f(x)); }
fn inc(n: i64) i64 { return n + 1; }
fn main() i64 {
    const add = fn (a, b) { return a + b; };
    print_int(add(1, 2));

    const base = 100;
    const bump = fn (x) { return x + base; };
    print_int(bump(5));

    print_int(twice(inc, 5));
    return 0;
}
