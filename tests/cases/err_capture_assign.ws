// error: which this closure captured
fn main() i64 {
    var count = 0;
    const bump = fn () void { count = count + 1; };
    bump();
    return count;
}
