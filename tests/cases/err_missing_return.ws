// error: not every path returns a value
fn f(c: bool) i64 { if (c) { return 1; } }
fn main() i64 { return f(true); }
