// A facade merging two overload sets under one name.
//
// `pub const x = other.x;` is a second key holding the same thing, and a facade
// binds one name to one thing -- so a package whose `render` lives in two of
// its files could not present one `render`. What it had to do instead was
// forward by hand, a function per member, which works and says what the
// re-export already says.
//
// An overload set is exactly what "only functions may share a name" promises,
// and two re-exports of one name are two names for two sets. So they merge.
// Only when the name is already a re-export: a re-export beside a declaration
// of this module's own is a module shadowing its own name with a foreign one,
// and stays a clash -- `err_reexport_clash.ws` is that.
// expect: dot
// expect: big dot
// expect: line
// expect: dot
// expect: big dot
// expect: big dot
const shapes = @import("./modules/mergefacade.ws");

fn main() i64 {
    // Every member of both sets, through the one name.
    print(shapes.render(shapes.Dot{ }));
    print(shapes.render(shapes.BigDot{ }));
    print(shapes.render(shapes.Line{ .n = 1 }));

    // The facade's own use of the merged name.
    print(shapes.describe(shapes.Dot{ }));
    print(shapes.describe(shapes.BigDot{ }));

    // And it is dispatch rather than a static pick: held at the supertype, a
    // `BigDot` still finds its own overload.
    const held: shapes.Dot = shapes.BigDot{ };
    print(shapes.render(held));
    return 0;
}
