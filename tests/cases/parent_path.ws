// A supertype named through the module that declares it.
//
// `struct Sub : Base` resolved `Base` as a bare name, which made this the one
// position in the language where a type could not be reached through a module.
// A subtype of a type another module declares -- including one a package facade
// re-exports -- had to be declared inside that module, while every other
// position already took `pkg.Base`.
//
// The parent is a whole type expression now, resolved exactly as any other type
// position resolves one. Aliasing runs before parents do, so the facade's name
// works as well as the original: `shapes.Point` and `shapesinner.Point` are one
// `StructId`, which is what makes `Labelled` and `Tagged` below siblings under
// one type rather than subtypes of two.
// expect: 25
// expect: 25
// expect: point
// expect: point
// expect: labelled
// expect: tagged
// expect: plain
const shapes = @import("./modules/shapes.ws");
const inner = @import("./modules/shapesinner.ws");

// Through the facade, and through the file the facade presents. The same type
// either way.
const Labelled = struct : shapes.Point { tag: str };
const Tagged = struct : inner.Point { n: i64 };

// An overload set of this module's own, over a type declared in another.
fn describe(p: inner.Point) str { return "plain"; }
fn describe(l: Labelled) str { return "labelled"; }
fn describe(t: Tagged) str { return "tagged"; }

fn main() i64 {
    const l = Labelled{ .x = 3, .y = 4, .tag = "a" };
    const t = Tagged{ .x = 3, .y = 4, .n = 1 };

    // A subtype's fields are its supertype's followed by its own, so a
    // function compiled against `Point` reads them where it always did.
    print_int(shapes.norm2(l));
    print_int(inner.norm2(t));

    // The subtype really is in the lattice: a call written over there, over an
    // overload set that has never heard of these types, finds the supertype's
    // case rather than failing to dispatch.
    print(shapes.show(l));
    print(inner.show(t));

    // And a set written here dispatches on the subtypes themselves, through a
    // value held at the supertype.
    const a: inner.Point = l;
    const b: inner.Point = t;
    const c: inner.Point = inner.Point{ .x = 0, .y = 0 };
    print(describe(a));
    print(describe(b));
    print(describe(c));
    return 0;
}
