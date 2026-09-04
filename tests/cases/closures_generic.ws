// A `fn` literal bound to a `const` is a definition, not a value: it is
// generalised at its binding, and each use materialises a closure at the type
// that use needs. A closure value is one code pointer, and two instantiations
// need two -- which is the same reason an overload set bound to a `const` is a
// name rather than a value.
// expect: 7
// expect: a
// expect: 101
// expect: 6
// expect: 100
// expect: x
// expect: 9
// expect: still generic
// expect: 1
// expect: 0
// expect: 42
// expect: 7
// expect: 3
// expect: 3
const Box = struct[T] { value: T };

fn twice(f: fn(i64) i64, x: i64) i64 { return f(f(x)); }

fn outer[U](v: U) U {
    // A generic literal inside a generic function names its own `T` and the
    // enclosing `U` at once. Monomorphisation composes the two substitutions:
    // the caller's, which says what `U` is, and this use's, which says `T`.
    const wrap = fn [T](x: T) Box[T] { return Box{ .value = x }; };
    const b = wrap(v);
    const n = wrap(7);
    print_int(n.value);
    return b.value;
}

fn main() i64 {
    // Explicit type parameters, written exactly as a declaration writes them.
    const first = fn [T](a: []T) T { return a[0]; };
    print_int(first([]i64{ 7, 8 }));
    print(first([]str{ "a", "b" }));

    // A generic literal may capture. The captured value is shared by every
    // instantiation, because its type belongs to the enclosing frame and so is
    // never one of the quantified variables.
    const base = 100;
    const bump = fn [T](a: []T, i: i64) T { print_int(base + i); return a[i]; };
    print_int(bump([]i64{ 5, 6 }, 1));
    print(bump([]str{ "x", "y" }, 0));

    // Used as a value: the use instantiates it, and what is passed is an
    // ordinary closure of that one type.
    const id = fn (x) { return x; };
    print_int(twice(id, 9));
    print(id("still generic"));

    // Captures are by value, and the value is the one at the definition --
    // snapshotted there rather than re-read at each use.
    var seen = 1;
    const peek = fn (x) { print_int(seen); return x; };
    seen = 2;
    print_int(peek(0));

    // Named from inside another closure, which reaches past the frame boundary
    // for the definition and captures its snapshot along the way.
    const echo = fn (x) { return x; };
    const relay = fn (y) { return echo(y); };
    print_int(relay(42));

    print_int(outer(3));

    // A literal whose type a constraint still owns stays monomorphic, exactly
    // as `fn add(a, b) { return a + b; }` does: `Numeric` is solved with the
    // rest of the binding group, and quantifying a variable the solver is
    // about to pin would give the same body two answers.
    const add = fn (a, b) { return a + b; };
    print_int(add(1, 2));
    return 0;
}
