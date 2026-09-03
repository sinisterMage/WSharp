// error: missing field `y`
const Point = struct { x: i64, y: i64 };
fn main() i64 { const p = Point{ .x = 1 }; return p.x; }
