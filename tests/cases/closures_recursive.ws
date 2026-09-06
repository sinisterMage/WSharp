// A `fn` literal bound to a `const` may name itself.
//
// The name is bound to the literal's own closure value, and that value is
// already in hand: a literal is only ever entered *through* a closure, and a
// call passes that closure as the environment pointer. So the recursive
// reference costs a register rather than an allocation, and a generic literal
// recurses at one type per instantiation for free -- the closure the use site
// built already points at the specialisation the body is.
// expect: 120
// expect: 8
// expect: 3
// expect: 2
// expect: 6
// expect: 720
// expect: 55
const array = @import("std/array");

fn twice(f: fn(i64) i64, x: i64) i64 { return f(f(x)); }

fn main() i64 {
    const fact = fn (n) { if (n <= 1) { return 1; } return n * fact(n - 1); };
    print_int(fact(5));

    // A capture is visible to the recursive call too, because the environment
    // it recurses through is the one this call was entered with.
    const step = 2;
    const countdown = fn (n) { if (n <= 0) { return 0; } return step + countdown(n - step); };
    print_int(countdown(8));

    // Generic, and used at two types: each use materialises its own closure,
    // and each body recurses through the one it was given.
    const count = fn [T](a: []T, i: i64) i64 {
        if (i >= array.len(a)) { return 0; }
        return 1 + count(a, i + 1);
    };
    print_int(count([]i64{ 7, 8, 9 }, 0));
    print_int(count([]str{ "a", "b" }, 0));

    // Named from inside a nested closure, which reaches past the frame
    // boundary and captures the self reference like any other value.
    const sum = fn (n) {
        const rest = fn (m) { return sum(m); };
        if (n <= 0) { return 0; }
        return n + rest(n - 1);
    };
    print_int(sum(3));

    // Still an ordinary function value where one is wanted: `fact(fact(3))`.
    print_int(twice(fact, 3));

    const fib = fn (n) { if (n < 2) { return n; } return fib(n - 1) + fib(n - 2); };
    print_int(fib(10));
    return 0;
}
