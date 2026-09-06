// error: `&` needs an integer, but this is `f64`
// A bit pattern is not a thing to ask a float for.
fn main() i64 {
    const x = 1.5 & 2.0;
    return 0;
}
