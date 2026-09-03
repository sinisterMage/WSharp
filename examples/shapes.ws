// Structs and closures.
//
// Struct instances and closures are both heap objects with the same 16-byte
// header, allocated through the same `ws_alloc` entry point. That uniformity is
// what lets the collector trace a closure's captures exactly as it traces a
// struct's fields.

const Point = struct { x: i64, y: i64 };

const Rect = struct {
    origin: Point,
    width: i64,
    height: i64,
};

fn area(r: Rect) i64 {
    return r.width * r.height;
}

fn right_edge(r: Rect) i64 {
    return r.origin.x + r.width;
}

// A function taking another function. `twice` is generic in neither argument,
// but `apply_to_area` below shows a closure being passed the same way.
fn twice(f: fn(i64) i64, x: i64) i64 {
    return f(f(x));
}

fn main() i64 {
    var r = Rect{
        .origin = Point{ .x = 2, .y = 3 },
        .width = 10,
        .height = 4,
    };

    print_int(area(r));
    print_int(right_edge(r));

    // Fields are mutable through a `var` binding.
    r.width += 5;
    print_int(area(r));

    // A closure capturing a local by value.
    const margin = 100;
    const pad = fn (n) { return n + margin; };
    print_int(pad(area(r)));

    print_int(twice(pad, 0));
    return 0;
}
