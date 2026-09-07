// A facade: one file that presents a package of three.
//
// `pub const x = other.x;` is the whole mechanism. It costs no new syntax --
// that spelling already parsed, and was rejected as a computed global, which it
// is not: nothing is computed, because naming a name is not evaluating one.
//
// A facade states its surface, name by name, which is the same rule `pub`
// already sets: a module's surface is something it says rather than something
// it leaks. The imports below stay private, so the two files behind this one
// cannot be reached at all.
const inner = @import("./shapesinner.ws");
const walk = @import("./shapesiter.ws");

// A type. A struct's identity is its id, so this is a second name for the same
// type and not a copy of it: `shapes.Point` and `shapesinner.Point` unify,
// dispatch and lay out identically because they are one type.
pub const Point = inner.Point;
// A type that is also a value, which needs both halves to arrive.
pub const Origin = inner.Origin;
// A constant.
pub const SCALE = inner.SCALE;
// One function, and an overload set that still dispatches on every member.
pub const norm2 = inner.norm2;
pub const show = inner.show;
// From the package's other file, so what a caller sees is one surface rather
// than the shape of the directory.
pub const Walk = walk.Walk;
pub const to = walk.to;
pub const scaled = walk.scaled;

// A facade may use what it re-exported, unqualified, exactly as it may use a
// name of its own: the key went into this module's own table.
pub fn diagonal(n: i64) i64 { return norm2(Point{ .x = n, .y = n }); }
