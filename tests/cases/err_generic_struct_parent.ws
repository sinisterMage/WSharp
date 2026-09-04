// error: a generic struct cannot have a supertype
const Base = struct { };
const Sub = struct[T] : Base { value: T };
fn main() i64 { return 0; }
