// An integer literal used at an `f64`.
//
// A literal is a `comptime_int` and takes the type its context asks for, which
// is what makes `0xff` a `u8` here and a `u32` there. `f64` was the one numeric
// type that did not count as asking, so `1.0` had to be written wherever an
// `f64` was wanted and `x + 1` on a float was a type error naming the literal.
//
// The type is settled once every substitution has been made, so the node
// becomes a float literal at monomorphisation -- which is what lets a body
// annotated `Number` hold one and be compiled at `f64` and at `i64` both.
//
// Only for a value an `f64` holds exactly. Above 2^53 the integers are no
// longer all representable, and rounding a constant somebody wrote, silently,
// is not something to do.
// expect: 2.5
// expect: 3.0
// expect: 1.0
// expect: 3.0
// expect: 3.0
// expect: 6
// expect: 0.5
// expect: 9007199254740992.0
// expect: -4.0
fn plus_one(x: f64) f64 { return x + 1; }

/// Compiled at both members, from one body holding one literal.
fn doubled(x: Number) { return x * 2; }

fn main() i64 {
    print_float(plus_one(1.5));

    const a: f64 = 3;
    print_float(a);

    var b = 0.0;
    b += 1;
    print_float(b);

    // The literal beside a float operand, either way round.
    print_float(2 * 1.5);
    print_float(doubled(1.5));
    print_int(doubled(3));

    // Division, where an integer reading would give a different answer: `1 / 2`
    // is 0 and `1.0 / 2.0` is 0.5.
    const half: f64 = 1;
    print_float(half / 2);

    // The largest integer an `f64` holds exactly.
    const big: f64 = 9007199254740992;
    print_float(big);

    // A literal on the left of a float operand, which settles the same way.
    print_float(0 - 4.0);
    return 0;
}
