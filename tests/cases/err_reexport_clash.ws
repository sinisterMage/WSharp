// A renamed name is a name this module has, so it collides with one this
// module declares -- and says so where a reader can act on it, rather than
// silently taking one of the two.
//
// The type is the interesting half: a type re-export and a `struct` are
// answered by different passes, and the second has to know the first declined.
// error: `Point` is declared more than once
// error: `norm2` is declared more than once
const inner = @import("./modules/shapesinner.ws");

pub const Point = struct { z: i64 };
pub const Point = inner.Point;

fn norm2(n: i64) i64 { return n; }
pub const norm2 = inner.norm2;

fn main() i64 { return 0; }
