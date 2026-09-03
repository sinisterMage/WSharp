// error: needs an integer
// `%` has no float form: Cranelift has no float remainder and the language
// does not define one, so inference must reject it rather than let code
// generation trip over it.
fn main() i64 {
    const r = 1.5 % 0.5;
    return 0;
}
