// error: cannot assign to `x`, which is `const`
fn main() i64 {
    for ([]i64{ 1, 2 }) |x| { x = 3; }
    return 0;
}
