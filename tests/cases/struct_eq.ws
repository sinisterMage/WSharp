// expect: two equal points
// expect: a different y is a different point
// expect: a value equals itself
// expect: names and nested structs compare
// expect: a different name is a different row
// expect: two equal circles compared at Shape
// expect: circles differing only in r are not equal
// expect: a circle is never a square
// expect: an absent field differs from a present one
// expect: two absent fields agree
// expect: NaN makes a value unequal to itself
// expect: a generic struct compares at its instantiation
// `==` on a struct: field by field, recursing into struct fields.
//
// Test assertions compare structs constantly, and until now that meant writing
// the comparison out. What `==` means here is stated rather than left to be
// discovered:
//
//  * the same object is equal to itself, whatever it holds;
//  * two values of *different* concrete types are never equal, so a `Circle`
//    and a `Square` compared at `Shape` are unequal without reaching a field;
//  * two values of the same concrete type compare **that type's** fields, so
//    two `Circle`s compared at `Shape` compare `r` -- comparing only what
//    `Shape` declares would call them equal, which is the mistake this dispatch
//    exists to avoid;
//  * a field compares by its own type's rule, so a `str` field compares by
//    contents and an `f64` field by `fcmp` -- which is why a `NaN` field makes
//    a value unequal to itself, exactly as a bare `f64` already does.
//
// A value that reaches itself recurses for ever. `a == a` stops at the identity
// test; `a.next = a; b.next = b; a == b` does not, and nothing detects it. That
// is what derived structural equality does everywhere it exists.
const Point = struct { x: i64, y: i64 };
const Row = struct { name: str, at: Point, weight: f64 };

const Shape = struct { sides: i64 };
const Circle = struct : Shape { r: i64 };
const Square = struct : Shape { side: i64 };

const Maybe = struct { it: ?i64 };
const Box = struct[T] { item: T };

/// Compiled against `Shape`, and called with subtypes: the comparison inside
/// is the dispatching one.
fn same_shape(a: Shape, b: Shape) bool { return a == b; }

fn main() i64 {
    const a = Point{ .x = 1, .y = 2 };
    const b = Point{ .x = 1, .y = 2 };
    const c = Point{ .x = 1, .y = 3 };
    if (a == b) { print("two equal points"); }
    if (a != c) { print("a different y is a different point"); }
    if (a == a) { print("a value equals itself"); }

    const p = Row{ .name = "one", .at = a, .weight = 1.5 };
    const q = Row{ .name = "one", .at = b, .weight = 1.5 };
    const r = Row{ .name = "two", .at = b, .weight = 1.5 };
    if (p == q) { print("names and nested structs compare"); }
    if (p != r) { print("a different name is a different row"); }

    const c1: Shape = Circle{ .sides = 0, .r = 5 };
    const c2: Shape = Circle{ .sides = 0, .r = 5 };
    const c3: Shape = Circle{ .sides = 0, .r = 9 };
    const s1: Shape = Square{ .sides = 4, .side = 5 };
    if (same_shape(c1, c2)) { print("two equal circles compared at Shape"); }
    if (!same_shape(c1, c3)) { print("circles differing only in r are not equal"); }
    if (!same_shape(c1, s1)) { print("a circle is never a square"); }

    const some = Maybe{ .it = 7 };
    const none = Maybe{ .it = null };
    const none2 = Maybe{ .it = null };
    if (some != none) { print("an absent field differs from a present one"); }
    if (none == none2) { print("two absent fields agree"); }

    const nan = Row{ .name = "one", .at = a, .weight = 0.0 / 0.0 };
    const nan2 = Row{ .name = "one", .at = a, .weight = 0.0 / 0.0 };
    if (nan != nan2) { print("NaN makes a value unequal to itself"); }

    const b1 = Box{ .item = "x" };
    const b2 = Box{ .item = "x" };
    const b3 = Box{ .item = "y" };
    if (b1 == b2 and b1 != b3) { print("a generic struct compares at its instantiation"); }
    return 0;
}
