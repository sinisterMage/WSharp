// error: `try` in a function that cannot fail
fn risky(n: i64) !i64 { return n; }
fn main() i64 { return try risky(1); }
