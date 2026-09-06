// expect: 21
// panic: integer division by zero
// Unsigned division needs the zero check and *not* the overflow one: there is
// no pair of unsigned values whose quotient does not fit, so that branch is
// not emitted at all.
fn div(a: u32, b: u32) u32 { return a / b; }
fn main() i64 {
    print_int(i64(div(85, 4)));
    print_int(i64(div(1, 0)));
    return 0;
}
