// Arithmetic that is not an operator.
//
// `min` and `max` are written once over the abstract type `Number`, because
// comparing two numbers says nothing about which kind they are: each is a
// constrained generic, compiled once per type it is used at.
//
// `abs` and `sign` cannot be, and the reason changed when item 9 landed. It
// used to be that they compare against a literal zero and an integer literal
// was an `i64`, so a single definition would pin `Number` to `i64` at the
// comparison; `comptime_int` retired that. What stands instead is that both
// negate, and negation is meaningless on an unsigned type -- which `Number`
// includes. They are an overload set for that reason, and a narrow signed
// value needs a conversion to use one.
//
// The rest are `f64` machine instructions and come from the builtin table.

pub fn min(a: Number, b: Number) {
    if (a < b) { return a; }
    return b;
}

pub fn max(a: Number, b: Number) {
    if (a > b) { return a; }
    return b;
}

pub fn abs(x: i64) i64 {
    if (x < 0) { return -x; }
    return x;
}

pub fn abs(x: f64) f64 {
    if (x < 0.0) { return -x; }
    return x;
}

/// The sign of `x`: -1, 0 or 1.
pub fn sign(x: i64) i64 {
    if (x < 0) { return -1; }
    if (x > 0) { return 1; }
    return 0;
}

pub fn sign(x: f64) f64 {
    if (x < 0.0) { return -1.0; }
    if (x > 0.0) { return 1.0; }
    return 0.0;
}

/// `base` raised to a non-negative integer power.
///
/// Integer-only: `pow` beside it is the floating-point one, and rounding an
/// integer through a float is a surprise waiting to happen.
pub fn ipow(base: i64, exponent: i64) i64 {
    var result = 1;
    var i = 0;
    while (i < exponent) : (i += 1) { result = result * base; }
    return result;
}
