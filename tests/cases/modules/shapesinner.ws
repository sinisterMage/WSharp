// The inside of a package: what a facade would rather nobody imported
// directly. Nothing here says it is one file of several -- that is the whole
// point, since a package presents its facade and the loader offers no way to
// name anything else in it.
pub const Point = struct { x: i64, y: i64 };

// A struct with no fields is a type *and* a value, so a re-export of one has to
// carry both. `Origin` is the case that would pass while only half working.
pub const Origin = struct {};

pub const SCALE = 10;

pub fn norm2(p: Point) i64 { return p.x * p.x + p.y * p.y; }

// An overload set, so that a name re-exported once still dispatches on every
// member: the set moved names, not code.
pub fn show(p: Point) str { return "point"; }
pub fn show(o: Origin) str { return "origin"; }
pub fn show(n: i64) str { return "number"; }

// The inside of the inside. A facade may not re-export this, and
// `err_reexport_private.ws` is what checks that it may not.
fn secret() i64 { return 99; }
pub fn call_secret() i64 { return secret(); }
