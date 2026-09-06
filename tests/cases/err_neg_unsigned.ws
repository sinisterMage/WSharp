// error: `-` needs a signed number, but this is `u8`
// error: `-` needs a signed number, but `Number` includes `u8`
// `-x` on an unsigned type is not an error the machine reports: it is
// 256 - x, which is exactly the kind of wrong that survives to production.
// The second one is the same rule reaching an abstract parameter, which must
// work for every type it lists.
fn flip(x: u8) u8 { return -x; }
fn negate(n: Number) { return -n; }
fn main() i64 { return 0; }
