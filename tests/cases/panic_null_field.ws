// expect: 2
// panic: read through a null reference
//
// An element of `array.new(n)` whose type is a struct is null until it is
// assigned, and reading a field of it used to be a segfault with no W#
// diagnostic (#67's class). The read is allowed to fault and the fault is
// reported: see `wsharp_runtime::trap`.
const array = @import("std/array");

const Point = struct { x: i64, y: i64 };

fn main() i64 {
    var ps: []Point = array.new(2);
    print(array.len(ps));
    ps[1].y = 4;
    print("unreachable");
    return 0;
}
