// A top-level `const` array can be written through an alias, and this case
// asserts that it can -- which is the uncomfortable shape a guard for a
// *documented limitation* has. LIMITATIONS.md, "A top-level `const` array can be
// written through an alias", is the entry; this is what stops it silently
// stopping being true.
//
// Writing an element through the `const`'s own name is refused
// (`err_assign_const_array.ws`). The check is syntactic -- it is on the name at
// the assignment -- so binding the same array to a local and writing through
// that goes through, and the table every worker in the process shares changes.
//
// The reason it is documented rather than fixed is that the property being
// checked is not a property of the *name*: it belongs to the reference, and W#
// has no way to say that a reference is read-only. Moving the check to the
// binding -- refusing `var a = K;` -- would close this spelling and not the next
// one, because a function parameter is an alias too:
//
//     fn zero(a: []i64) void { a[0] = 0; }
//     zero(K);
//
// A check that stops the first and not the second is worse than the one that is
// here, because it reads like a guarantee.
//
// The data is emitted *writable* on purpose (`wsharp-codegen/src/lib.rs` --
// `define_arrays`): a read-only page would turn this mistake into a fault with
// no message, and a wrong answer with a documented cause beats a SIGSEGV.
//
// Two tables rather than one, so that the half showing the copy is not reading
// something the half showing the hole already changed.
// expect: 99
// expect: 99
// expect: 1
// expect: 42
// expect: 3
const array = @import("std/array");

/// Written through an alias, below.
const HOLE = []i64{ 1, 2, 3 };
/// Copied before being written, which is the workaround.
const KEPT = []i64{ 1, 2, 3 };

fn main() i64 {
    // The limitation. `a` and `HOLE` are one object, so this is a write to the
    // data section, and every later reader of `HOLE` sees it.
    var a = HOLE;
    a[0] = 99;
    print(HOLE[0]);
    print(a[0]);

    // The workaround: copy, then change the copy. `array.slice` allocates, so
    // `copy` is an ordinary heap array the collector traces and the caller owns.
    var copy = array.slice(KEPT, 0, array.len(KEPT));
    copy[0] = 42;
    print(KEPT[0]);
    print(copy[0]);

    // And the copy really is a copy, not a view: its length is the slice's.
    print(array.len(copy));
    return 0;
}
