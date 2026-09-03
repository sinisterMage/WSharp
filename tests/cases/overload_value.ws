// expect: 1
// expect: 2
// expect: 2
// An overload set with an annotation that pins one signature is a function
// value: one code pointer, chosen once, with no dispatch left in it.
//
// `wide` is the `Base` member, so calling it with a `Sub` runs the `Base` body
// -- the annotation, not the argument, decided. That is exactly the difference
// between a value and a call to the set itself (see overload_alias.ws).
const Base = struct { };
const Sub = struct : Base { };

fn size(x: Base) i64 { return 1; }
fn size(x: Sub) i64 { return 2; }

const wide: fn(Base) i64 = size;

fn apply(f: fn(Sub) i64) i64 { return f(Sub); }

fn main() i64 {
    print_int(wide(Sub));

    const narrow: fn(Sub) i64 = size;
    print_int(narrow(Sub));
    // And it really is a value: it can be passed to something expecting one.
    print_int(apply(narrow));
    return 0;
}
