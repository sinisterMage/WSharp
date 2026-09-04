// expect: 3
// expect: 2.5
// expect: 3
// expect: 1.5
// expect: 1.4142135623730951
// expect: 2.0
// expect: 3.0
// expect: 1024
// expect: -1
// expect: 1.0
// `abs` and `sign` are overload sets rather than one function over the
// abstract type `Number`, because they compare against a literal zero and an
// integer literal is an `i64`.
const math = @import("std/math");
fn main() i64 {
    print_int(math.abs(-3));
    print_float(math.abs(-2.5));
    print_int(math.min(3, 7));
    print_float(math.max(1.5, 0.5));
    print_float(math.sqrt(2.0));
    print_float(math.floor(2.7));
    print_float(math.ceil(2.1));
    print_int(math.ipow(2, 10));
    print_int(math.sign(-9));
    print_float(math.sign(0.5));
    return 0;
}
