// expect: 1
// panic: integer division by zero
// `%` shares the zero check with `/`. `i64::MIN % -1` needs no check of its
// own: its result is 0, and Cranelift defines it so.
fn rem(a: i64, b: i64) i64 { return a % b; }
fn main() i64 {
    print_int(rem(7, 3));
    print_int(rem(7, 0));
    return 0;
}
