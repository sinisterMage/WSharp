// expect: an integer
// expect: a number
// expect: 7
// expect: 1.5
// expect: 404+number
// expect: 404+int
// expect: status+number
// expect: status+int
// Dispatch on scalar types, through the abstract type `Number`.
//
// `Number` stands for the concrete types it lists, so an overload can be
// written for "any number" while a more specific one claims `i64`. Nothing is
// tested at run time: a scalar's type is always statically known, so both calls
// to `show` below are ordinary static calls.
//
// A parameter annotated with an abstract type is a *constrained generic*
// parameter, which is what gives it a machine representation: `bigger` is
// compiled once per pair of types it is used at, and `>` inside it does not
// decide for the caller.
fn show(x: i64) str { return "an integer"; }
fn show(x: Number) str { return "a number"; }

fn bigger(a: Number, b: Number) { if (a > b) { return a; } return b; }

// Both kinds of specificity in one table. `s` is statically a `Status`, so its
// position is decided by the type id in the object's header; `n`'s type is
// already known, so its position costs nothing at all -- and the two orderings
// compose, which is why all four overloads are needed to keep the calls
// unambiguous.
fn tag(s: Status, n: Number) str { return "status+number"; }
fn tag(s: NotFound404, n: Number) str { return "404+number"; }
fn tag(s: Status, n: i64) str { return "status+int"; }
fn tag(s: NotFound404, n: i64) str { return "404+int"; }

fn route(s: Status) void {
    print(tag(s, 1.5));
    print(tag(s, 7));
}

fn main() i64 {
    print(show(7));
    print(show(2.5));
    print_int(bigger(3, 7));
    print_float(bigger(1.5, 0.5));

    route(NotFound404);
    route(Ok200);
    return 0;
}
