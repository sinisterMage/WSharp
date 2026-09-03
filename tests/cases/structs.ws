// expect: 3
// expect: 4
// expect: 25
// expect: 11
const Point = struct { x: i64, y: i64 };
fn norm2(p: Point) i64 { return p.x * p.x + p.y * p.y; }
fn main() i64 {
    const p = Point{ .x = 3, .y = 4 };
    print_int(p.x);
    print_int(p.y);
    print_int(norm2(p));

    var q = Point{ .x = 1, .y = 2 };
    q.x = 8;
    q.y += 1;
    print_int(q.x + q.y);
    return 0;
}
