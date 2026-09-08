// error: `9007199254740993` is not exactly an `f64`
// An integer literal may be used at an `f64`, but only where the `f64` really
// is that integer. Not a range check: `f64` reaches far past `i64` and is exact
// for none of the top of it, so the test is the round trip. 2^53 is fine and
// 2^53 + 1 is the first that is not.
fn main() i64 {
    const a: f64 = 9007199254740993;
    print_float(a);
    return 0;
}
