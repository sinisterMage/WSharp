// Re-export: `pub const x = other.x;`, which is what lets a package of several
// files present one of them.
//
// A module could always be reached through the name that imported it, and
// nothing else: an `@import` binding is a module rather than a value, so a
// facade had no way to say "and this name means that one". That is the one
// thing item 11 needed from the type checker, and it turned out to be two more
// keys in tables that already existed -- `struct_ids` for a type and `globals`
// for everything else -- because a struct's identity is an id and a function's
// is a set of ids, and neither moved.
// expect: 25
// expect: 250
// expect: 10
// expect: point
// expect: origin
// expect: number
// expect: 8
// expect: 0
// expect: 1
// expect: 2
// expect: point
// expect: 99
const shapes = @import("./modules/shapes.ws");
const outer = @import("./modules/shapesouter.ws");
const inner = @import("./modules/shapesinner.ws");

fn main() i64 {
    // A re-exported type, named in an annotation and constructed.
    const p: shapes.Point = shapes.Point{ .x = 3, .y = 4 };
    print_int(shapes.norm2(p));
    // And used at a function that came from the package's *other* file, which
    // is the thing a facade is for: one surface over a directory.
    print_int(shapes.scaled(p));
    print_int(shapes.SCALE);

    // An overload set re-exported once still dispatches on every member.
    print(shapes.show(p));
    print(shapes.show(shapes.Origin));
    print(shapes.show(1));

    print_int(shapes.diagonal(2));

    // `iter` and `next` are resolved in the module that declares the type, so
    // a re-exported one is iterable without the facade saying anything.
    for (shapes.to(3)) |i| { print_int(i); }

    // A facade over a facade.
    const q: outer.Point = outer.Point{ .x = 1, .y = 1 };
    print(outer.show(q));

    // The same type by both names, which is what "a second name" has to mean:
    // this would not typecheck if the alias were a copy.
    const same: inner.Point = q;
    print_int(inner.call_secret() + same.x - 1);
    return 0;
}
