// Arithmetic that is not an operator.
//
// `min` and `max` are written once over the abstract type `Number`, because
// comparing two numbers says nothing about which kind they are: each is a
// constrained generic, compiled once per type it is used at.
//
// `abs` and `sign` are written over `Signed`, which is the four signed integer
// types. Not `Number` and not `Integer`: both bodies negate, and negation is
// meaningless on an unsigned type -- `-x` on a `u8` is 256 - x, which is the
// kind of wrong that survives to production. That is what kept these an
// `i64`/`f64` overload set until there was an abstract type saying so, and why
// a narrow signed value used to need a conversion to use one.
//
// `f64` is not in `Signed` and so keeps an overload of its own here. The two
// are disjoint, so there is nothing to be ambiguous about; what stops the
// generic one covering both is the literal zero each body compares against,
// which is an integer literal and has no `f64` reading yet.
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

/// `x` without its sign.
///
/// One definition for every signed integer width, compiled once per width it
/// is used at. `abs(i64::MIN)` is `i64::MIN`, as it is everywhere else here:
/// `-` wraps, and the one value with no positive counterpart cannot be given
/// one.
pub fn abs(x: Signed) {
    if (x < 0) { return -x; }
    return x;
}

pub fn abs(x: f64) f64 {
    if (x < 0.0) { return -x; }
    return x;
}

/// The sign of `x`: -1, 0 or 1.
pub fn sign(x: Signed) {
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
