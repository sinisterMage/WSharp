// Visibility. A module's names are private to it unless it writes `pub`, and
// an unqualified name is unaffected either way: it can only mean this module's
// own or the prelude's, and both are always visible.
// expect: 3
// expect: 6
// expect: 100
// expect: 1
// expect: 42
const vis = @import("./modules/vis.ws");

// This module's own `helper`, which is a different function from the private
// one `vis` declares -- names are stored qualified, so the two never met.
fn helper() i64 { return 1; }

fn main() i64 {
    const c = vis.start();
    print_int(vis.bump(c));
    print_int(vis.bump(c));

    // `vis.helper` is private, but `vis` may name it itself.
    print_int(vis.call_helper());
    print_int(helper());

    // A public type is nameable, and its fields are the struct's business
    // rather than the module system's.
    var mine: vis.Counter = vis.Counter{ .n = 42 };
    print_int(mine.n);
    return 0;
}
