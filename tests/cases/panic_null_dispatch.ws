// expect: 9
// panic: read through a null reference
//
// A dispatched call reads its argument's type id out of the header, so a null
// argument is a read through null too -- found by the header read, before any
// overload is chosen.
const array = @import("std/array");

const Shape = struct {};
const Square = struct : Shape { side: i64 };
const Circle = struct : Shape { r: i64 };

fn area(s: Square) i64 { return s.side * s.side; }
fn area(c: Circle) i64 { return c.r * c.r * 3; }

fn main() i64 {
    var shapes: []Shape = array.new(2);
    const sq: Shape = Square{ .side = 3 };
    shapes[0] = sq;
    print(area(shapes[0]));
    print(area(shapes[1]));
    print("unreachable");
    return 0;
}
