// expect: 7
// expect: 65535
// expect: -3
// expect: 2
// expect: str
// A conversion inside an abstract-typed body must not settle the parameter.
//
// A parameter annotated `Integer` is a *constrained generic*: the caller picks
// which member it is compiled at, and `i64(v)` is valid at every one of them.
// Settling it to `i64` here -- which is what the "an unannotated argument would
// otherwise stay a variable for ever" default used to do -- made `show` a
// monomorphic `fn(i64)` while its declaration still said `Integer`. With one
// overload that surfaced as a bogus `this argument has type u8, expected i64`
// at the call; with a second overload beside it, `overlap` kept the case on the
// strength of the *declared* type, threw away the failed unification, and
// compiled a static call to a body expecting an `i64`. A `u8` then reached
// Cranelift where an `i64` was declared, and the verifier rejected the result.
//
// Overload resolution must not be able to produce IR that fails verification,
// so both halves are fixed and both are exercised here: the set below has two
// members, and the `Integer` one is called at four widths.
const text = @import("std/str");

fn show(v: Integer) void { print(text.from_int(i64(v))); return; }
fn show(v: str) void { print(v); return; }

fn main() i64 {
    const a: u8 = 7;
    const b: u16 = 65535;
    const c: i32 = -3;
    const d: i64 = 2;
    show(a);
    show(b);
    show(c);
    show(d);
    show("str");
    return 0;
}
