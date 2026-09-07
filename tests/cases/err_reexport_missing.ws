// A `const` bound to a name in another module says which module, so the
// failure can too -- rather than the "must be a literal or a `fn`" this used to
// be, which sent the reader looking at the wrong half of the line.
//
// The second one is the case that has to keep working: an expression really is
// a computed global, and that message is still the right one.
// error: `inner` has nothing called `norm3`
// error: a top-level `const` must be a literal or a `fn`
const inner = @import("./modules/shapesinner.ws");

pub const norm3 = inner.norm3;
pub const TWICE = inner.SCALE * 2;

fn main() i64 { return 0; }
