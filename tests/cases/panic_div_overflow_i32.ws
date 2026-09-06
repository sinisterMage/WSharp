// expect: -2147483648
// panic: integer overflow in division
// The one division whose result does not fit, at a width that is not `i64`:
// the check is per type, not against `i64::MIN`.
fn div(a: i32, b: i32) i32 { return a / b; }
fn main() i64 {
    const min: i32 = -2147483648;
    print_int(i64(min));
    print_int(i64(div(min, -1)));
    return 0;
}
