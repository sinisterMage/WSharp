// Assigning through a computed base: `g[i][j] = v`.
//
// This was rejected, and for a real reason: a compound assignment is checked
// and lowered as `x = x <op> e`, so the target's base appears twice, and
// re-evaluating `g[i]` is a second index -- or a second call, if the base
// computes one. Restricting the base to a variable or a field chain made that
// unobservable, and left `var row = g[i]; row[j] = v;` as the spelling, which
// is correct rather than merely accepted, since an array is a reference.
//
// So the base is evaluated once, into a local nothing can name, and the place
// is built on that. Re-reading a local is free, which is what made the
// restriction unnecessary rather than merely inconvenient. A base that is
// already a variable or a field chain is left exactly as it was.
// expect: 1
// expect: 2
// expect: 3
// expect: 14
// expect: 8
// expect: evaluated
// expect: 11
// expect: 1
// expect: 9
const array = @import("std/array");

const Cell = struct { n: i64 };
const Counter = struct { n: i64 };

/// Counts its calls and says so, to prove it runs once.
fn bump(c: Counter) i64 {
    c.n += 1;
    print("evaluated");
    return 0;
}

fn main() i64 {
    var g: [][]i64 = array.new(2);
    g[0] = array.new(2);
    g[1] = array.new(2);
    g[0][0] = 1;
    g[0][1] = 2;
    g[1][0] = 3;
    g[1][1] = 4;
    g[1][1] += 10;
    print_int(g[0][0]);
    print_int(g[0][1]);
    print_int(g[1][0]);
    print_int(g[1][1]);

    // A field of an indexed element, plain and compound.
    var cells: []Cell = array.new(1);
    cells[0] = Cell{ .n = 5 };
    cells[0].n = 7;
    cells[0].n += 1;
    print_int(cells[0].n);

    // The base has a side effect, and a compound assignment is where a second
    // evaluation would show. One line, one increment.
    const c = Counter{ .n = 0 };
    g[bump(c)][0] += 10;
    print_int(g[0][0]);
    print_int(c.n);

    // An array is a reference, so the old spelling still means the same thing
    // and still works.
    var row = g[1];
    row[0] = 9;
    print_int(g[1][0]);
    return 0;
}
