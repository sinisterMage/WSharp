// expect: 1
// expect: 2
// expect: 2
// expect: 1
// A `const` bound to an overload set with no annotation is a second name for
// the whole set: every call through it dispatches exactly as the original does,
// statically where the argument's type settles it and on the runtime type id
// where it does not.
const Base = struct { };
const Sub = struct : Base { };

fn size(x: Base) i64 { return 1; }
fn size(x: Sub) i64 { return 2; }

const measure = size;

// `x` is statically a `Base`, so this one is decided by the header at run time.
fn measure_any(x: Base) i64 { return measure(x); }

fn main() i64 {
    print_int(measure(Base));
    print_int(measure(Sub));

    const m = size;
    print_int(m(Sub));
    print_int(measure_any(Base));
    return 0;
}
