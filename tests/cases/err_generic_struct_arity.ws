// error: takes 2 type arguments, but 1 were given
const Pair = struct[A, B] { first: A, second: B };
fn f(p: Pair[i64]) i64 { return p.second; }
fn main() i64 { return 0; }
