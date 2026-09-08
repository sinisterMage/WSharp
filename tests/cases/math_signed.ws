// `math.abs` and `math.sign` over any signed integer width.
//
// They were an `i64`/`f64` overload set, because both negate and negation is
// meaningless on an unsigned type -- so `Number` and `Integer` were each too
// wide to write them over, and a narrow signed value needed a conversion at
// every call. `Signed` is the abstract type that says what they actually
// accept: the four signed integer widths, one definition compiled once per
// width it is used at.
//
// `f64` keeps an overload of its own, disjoint from that one, so nothing here
// is ambiguous.
// expect: 7
// expect: 70000
// expect: 300
// expect: -1
// expect: 1
// expect: 0
// expect: 5
// expect: 2.5
// expect: -1
// expect: -1.0
// expect: -128
const math = @import("std/math");

fn main() i64 {
    const a: i8 = -7;
    const b: i32 = -70000;
    const c: i16 = -300;
    print_int(i64(math.abs(a)));
    print_int(i64(math.abs(b)));
    print_int(i64(math.abs(c)));
    print_int(i64(math.sign(a)));
    print_int(i64(math.sign(i16(9))));
    print_int(i64(math.sign(i32(0))));

    // An unannotated literal still settles to `i64` and picks the same
    // definition at that width.
    print_int(math.abs(-5));
    print_float(math.abs(-2.5));
    print_int(math.sign(-9));
    print_float(math.sign(-9.5));

    // The one value with no positive counterpart. `-` wraps here as it does
    // everywhere else, so this is `i8`'s minimum unchanged rather than a trap.
    const low: i8 = -128;
    print_int(i64(math.abs(low)));
    return 0;
}
