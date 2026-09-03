// error: no overload of `size` has type `fn(Base) str`
// The annotation on a `const` bound to an overload set must be one member's
// signature exactly -- it names the code pointer to take, so there is nothing
// to widen or convert. Neither member returns `str`.
const Base = struct { };
const Sub = struct : Base { };

fn size(x: Base) i64 { return 1; }
fn size(x: Sub) i64 { return 2; }

const bad: fn(Base) str = size;

fn main() i64 { return 0; }
