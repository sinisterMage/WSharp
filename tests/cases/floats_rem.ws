// `%` on `f64`.
//
// Cranelift has no float remainder, so this was rejected by inference rather
// than emulated, and `Integer` existed partly to be the abstract type a body
// needing `%` could claim. It is a call now -- the same shape `str ==` has --
// so `%` asks for a number like the other four arithmetic operators, and only
// the bit operators still ask for an integer.
//
// Truncated, so the result takes the sign of the *dividend* and is smaller than
// the divisor in magnitude. That is C's `fmod` and every other language's that
// has one, and it is what makes `-7.5 % 2.0` equal `-1.5` rather than `0.5`.
// Checked against Python's `math.fmod`, which is a second implementation.
// expect: 1.5
// expect: -1.5
// expect: 1.5
// expect: -1.5
// expect: 0.5
// expect: 1.5
// expect: 1.0
// expect: 0.0
// expect: 2.0
const math = @import("std/math");

fn main() i64 {
    print_float(5.5 % 2.0);
    print_float(-7.5 % 2.0);
    print_float(7.5 % -2.0);
    print_float(-7.5 % -2.0);
    print_float(0.5 % 2.0);

    // `%=` arrives here too, as `x = x % e`.
    var x = 10.5;
    x %= 3.0;
    print_float(x);

    // The one that says this is a real `fmod` rather than the obvious inline
    // form. `10000000000000000.0 / 3.0` cannot be represented exactly, so
    // `a - trunc(a / b) * b` answers 0.0 where the true remainder is 1.0.
    print_float(10000000000000000.0 % 3.0);

    // An exact division has no remainder, and keeps the dividend's sign.
    print_float(6.0 % 3.0);

    // The same function under its own name, which is what the operator calls.
    print_float(math.rem(8.0, 3.0));
    return 0;
}
