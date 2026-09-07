// A facade over a facade, because a package may depend on a package and
// present what it was given. Resolution therefore runs to a fixpoint rather
// than in one pass: this file's answer is not knowable until `shapes` has one.
const shapes = @import("./shapes.ws");

pub const Point = shapes.Point;
pub const show = shapes.show;
