// error: type mismatch: the right operand has type `u32`, expected `u8`
// No promotion: a silent widening is how a 32-bit hash becomes a 64-bit one
// that is right for a while. `u32(a) + b` is how it is written.
fn add(a: u8, b: u32) u32 { return a + b; }
fn main() i64 { return i64(add(1, 2)); }
