// Aliasing does not bypass field-wise equality, including nested optionals
// and fields found by dispatching through a supertype.
// expect: false
// expect: false
// expect: false
// expect: true
// expect: false
// expect: false
// expect: false
// expect: false
// expect: false
// expect: true
// expect: true
// expect: true
// expect: true
// expect: false
// expect: false
// expect: true
const Box = struct { x: f64 };
const Nested = struct { value: Box };
const Optional = struct[T] { value: ?T };
const Base = struct { id: i64 };
const Derived = struct : Base { x: f64 };

fn equal(a: Base, b: Base) bool { return a == b; }

fn main() void {
    const nan = 0.0 / 0.0;
    const a = Box{ .x = nan };
    const b = Box{ .x = nan };
    print(nan == nan);
    print(a == a);
    print(a == b);
    print(a != a);

    const nested = Nested{ .value = a };
    print(nested == nested);
    print(nested == Nested{ .value = a });
    print(nested == Nested{ .value = b });

    const opt: Optional[f64] = Optional{ .value = nan };
    print(opt == opt);
    const opt_box: Optional[Box] = Optional{ .value = a };
    print(opt_box == opt_box);
    const none: Optional[f64] = Optional{ .value = null };
    print(none == none);
    const none2: Optional[f64] = Optional{ .value = null };
    print(none == none2);
    print(opt != none);

    const finite = Box{ .x = 1.0 };
    print(finite == finite);
    const sub: Base = Derived{ .id = 1, .x = nan };
    print(equal(sub, sub));
    print(equal(sub, Derived{ .id = 1, .x = nan }));
    print(Box{ .x = 0.0 } == Box{ .x = -0.0 });
}
