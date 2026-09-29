// A worker whose methods fault, for `panic_worker_null.ws` and
// `panic_worker_stack_overflow.ws`: a fault on a worker's thread is the
// program's as much as one on the main thread is, and is reported the same way.
const array = @import("std/array");

pub const Point = struct { x: i64, y: i64 };
pub const State = struct { points: []Point };

pub fn init(n: i64) State {
    const points: []Point = array.new(n);
    return State{ .points = points };
}

pub fn first_x(s: State) i64 { return s.points[0].x; }

fn down(n: i64) i64 { return down(n + 1) + 1; }

pub fn recurse(s: State) i64 { return down(array.len(s.points)); }
