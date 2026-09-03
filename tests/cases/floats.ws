// expect: 1.0
// expect: -2.5
// expect: -3.0
// expect: 0.30000000000000004
// expect: 2.5
// expect: 0.5
// expect: 3.5
// expect: true
// expect: false
// expect: true
// expect: 3.75
// expect: 1.5
// expect: 0.0
// Whole values print with one decimal so they cannot be mistaken for
// integers; everything else prints the shortest digits that round-trip, which
// is why `0.1 + 0.2` shows its famous tail.
const Circle = struct { radius: f64 };
fn half(x: f64) ?f64 {
    if (x < 0.0) { return null; }
    return x / 2.0;
}
fn main() i64 {
    print_float(1.0);
    print_float(-2.5);
    print_float(-3.0);
    print_float(0.1 + 0.2);
    print_float(10.0 / 4.0);
    print_float(2.0 - 1.5);
    var acc = 0.5;
    acc += 1.25;
    acc /= 0.5;
    print_float(acc);
    print_bool(1.5 < 2.0);
    print_bool(2.0 == 3.0);
    print_bool(-1.0 >= -1.0);
    const c = Circle{ .radius = 1.5 };
    print_float(c.radius * 2.5);
    print_float(half(3.0) orelse 0.0);
    print_float(half(-1.0) orelse 0.0);
    return 0;
}
