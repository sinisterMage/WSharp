// error: `%` needs an integer, but `Number` includes `f64`
// `Number` lists every numeric type now, so a body annotated with it has to
// work for `f64` as well as for `u8`. `Integer` is what a body that needs `%`
// claims instead.
fn odd(n: Number) { return n % 2; }
fn main() i64 { return 0; }
