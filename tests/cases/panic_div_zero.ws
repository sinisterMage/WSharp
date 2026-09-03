// expect: before
// panic: integer division by zero
// A zero divisor used to be a Cranelift trap -- a SIGILL with no message.
fn div(a: i64, b: i64) i64 { return a / b; }
fn main() i64 {
    print("before");
    print_int(div(1, 0));
    print("after");
    return 0;
}
