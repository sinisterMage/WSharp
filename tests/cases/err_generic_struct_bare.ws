// error: takes 1 type argument, but 0 were given
const Box = struct[T] { value: T };
fn f(b: Box) i64 { return 0; }
fn main() i64 { return 0; }
