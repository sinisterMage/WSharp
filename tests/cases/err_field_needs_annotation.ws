// error: cannot tell which type the field `x` belongs to
const Point = struct { x: i64, y: i64 };
fn getx(p) { return p.x; }
fn main() i64 { return getx(Point{ .x = 1, .y = 2 }); }
