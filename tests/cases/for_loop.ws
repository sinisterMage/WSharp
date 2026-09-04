// `for` over an array, with and without the index, and the loop controls.
// expect: 10
// expect: 20
// expect: 30
// expect: 10
// expect: 120
// expect: 230
// expect: 10
// expect: a
// expect: b
// expect: 40
// expect: 6
fn main() i64 {
    const xs = []i64{ 10, 20, 30 };
    for (xs) |x| { print_int(x); }
    for (xs) |x, i| { print_int(i * 100 + x); }

    var total = 0;
    for (xs) |x| {
        // `continue` still advances the loop, because the step is the
        // continue expression rather than the last statement of the body.
        if (x == 20) { continue; }
        if (x == 30) { break; }
        total = total + x;
    }
    print_int(total);

    const names = []str{ "a", "b" };
    for (names) |n| { print(n); }

    // Nested loops each get their own hidden counter.
    for (xs) |a| {
        for (xs) |b| {
            if (a == 10 and b == 30) { print_int(a + b); }
        }
    }

    // An empty array runs the body zero times.
    var seen = 0;
    for ([]i64{}) |_unused| { seen = seen + 1; }
    print_int(seen + 6);
    return 0;
}
