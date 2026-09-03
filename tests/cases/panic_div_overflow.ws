// expect: -9223372036854775808
// panic: integer overflow in division
// `i64::MIN / -1` is the one division whose result does not fit. The literal
// `9223372036854775808` is itself out of range, so the value is built by
// subtraction.
fn div(a: i64, b: i64) i64 { return a / b; }
fn main() i64 {
    const min = -9223372036854775807 - 1;
    print_int(min);
    print_int(div(min, -1));
    return 0;
}
