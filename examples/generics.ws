// Explicit generic parameters, on functions, on structs, and on `fn`
// literals.
//
// All three are monomorphised: a generic function is compiled once per type it
// is used at, and a generic struct is laid out once per instantiation -- which
// it has to be, because a `?T` field is two slots or three depending on `T`.

const str = @import("std/str");

const Box = struct[T] { value: T };
const Pair = struct[A, B] { first: A, second: B };

fn unwrap[T](b: Box[T]) T { return b.value; }
fn swap[A, B](p: Pair[A, B]) Pair[B, A] {
    return Pair{ .first = p.second, .second = p.first };
}

// A recursive generic function. The call to `count` inside itself records no
// type arguments while its binding group is being inferred, and inference
// fills in the group's own variables once it has generalised.
fn count[T](a: []T, i: i64) i64 {
    if (i >= 3) { return 0; }
    return 1 + count(a, i + 1);
}

fn main() i64 {
    // A struct literal's type arguments come from its field values.
    const b = Box{ .value = 42 };
    print_int(unwrap(b));
    print(unwrap(Box{ .value = "boxed" }));

    const p = Pair{ .first = 1, .second = "one" };
    print(swap(p).first);
    print_int(swap(p).second);

    // An annotation reaches the literal's fields, so `5` coerces into `?i64`
    // exactly as it would in any other annotated binding.
    const maybe: Pair[?i64, str] = Pair{ .first = 5, .second = "five" };
    print_int(maybe.first orelse -1);

    print_int(count([]i64{ 1, 2, 3 }, 0));
    print(str.from_int(count([]str{ "a", "b", "c" }, 0)));

    // A `const` bound to a `fn` literal is a *definition*, not a value, so it
    // generalises exactly as a declaration does. It has to be: a closure value
    // is one code pointer, and these two uses need two.
    const first = fn [T](a: []T) T { return a[0]; };
    print_int(first([]i64{ 7, 8 }));
    print(first([]str{ "seven", "eight" }));

    // Without written parameters it is the same rule, and it may capture --
    // the captured value is shared by every instantiation, because its type
    // belongs to this frame rather than to the literal.
    const tag = "picked ";
    const pick = fn (a, b, c) { if (c) { print(tag); return a; } return b; };
    print_int(pick(1, 2, true));
    print(pick("x", "y", true));

    // What a *constraint* still owns stays monomorphic, exactly as it does for
    // a declaration: `Numeric` is solved with the binding group, and `add` is
    // `fn(i64, i64) i64` for the same reason `fn add(a, b)` is.
    const add = fn (a, b) { return a + b; };
    print_int(add(2, 3));
    return 0;
}
