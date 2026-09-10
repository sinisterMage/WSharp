// expect: 255
// expect: 65535
// expect: 4294967295
// expect: 9007199254740993
// expect: -128
// expect: -32768
// expect: -2147483648
// expect: 1234
// expect: 3
// One `Integer` body, called at all eight widths, with a conversion inside it.
//
// The conversion is what used to pin the parameter, so the point of the case is
// that there is exactly one definition here and eight instantiations of it --
// `widen` is generic over the member the caller chose, and monomorphisation
// substitutes it. `sum` does the same with the conversion in an expression
// rather than in a return, and `round_trip` converts to `f64` and back, which
// is the other arm of the code generator's `convert`.
const text = @import("std/str");

fn widen(v: Integer) i64 { return i64(v); }
fn sum(a: Integer, b: Integer) i64 { return i64(a) + i64(b); }
fn round_trip(v: Number) i64 { return i64(f64(v)); }

fn main() i64 {
    const a: u8 = 255;
    const b: u16 = 65535;
    const c: u32 = 4294967295;
    // Above 2^53, which is what makes this a different question from the ones
    // an `f64` could have answered.
    const d: u64 = 9007199254740993;
    const e: i8 = -128;
    const f: i16 = -32768;
    const g: i32 = -2147483648;

    print_int(widen(a));
    print_int(widen(b));
    print_int(widen(c));
    print_uint(u64(widen(d)));
    print_int(widen(e));
    print_int(widen(f));
    print_int(widen(g));

    const h: u16 = 1200;
    const i: i8 = 34;
    print_int(sum(h, i));

    const j: f64 = 3.75;
    print_int(round_trip(j));
    return 0;
}
