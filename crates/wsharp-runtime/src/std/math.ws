// Arithmetic that is not an operator.
//
// `min` and `max` are written once over the abstract type `Number`, because
// comparing two numbers says nothing about which kind they are: each is a
// constrained generic, compiled once per type it is used at.
//
// `abs` and `sign` cannot be, and the reason is worth knowing: they compare
// against a literal zero, and an integer literal is an `i64`, so a single
// definition would pin `Number` to `i64` at the comparison. They are an
// overload set instead -- which is what the language is for.
//
// The rest are `f64` machine instructions and come from the builtin table.

fn min(a: Number, b: Number) {
    if (a < b) { return a; }
    return b;
}

fn max(a: Number, b: Number) {
    if (a > b) { return a; }
    return b;
}

fn abs(x: i64) i64 {
    if (x < 0) { return -x; }
    return x;
}

fn abs(x: f64) f64 {
    if (x < 0.0) { return -x; }
    return x;
}

/// The sign of `x`: -1, 0 or 1.
fn sign(x: i64) i64 {
    if (x < 0) { return -1; }
    if (x > 0) { return 1; }
    return 0;
}

fn sign(x: f64) f64 {
    if (x < 0.0) { return -1.0; }
    if (x > 0.0) { return 1.0; }
    return 0.0;
}

/// `base` raised to a non-negative integer power.
///
/// Integer-only: `pow` beside it is the floating-point one, and rounding an
/// integer through a float is a surprise waiting to happen.
fn ipow(base: i64, exponent: i64) i64 {
    var result = 1;
    var i = 0;
    while (i < exponent) : (i += 1) { result = result * base; }
    return result;
}
