// Indexed assignments capture the array and index before evaluating the RHS.
// expect: 1
// expect: 15
// expect: 20
// expect: 1
// expect: 15
// expect: 20
// expect: 2
// expect: 15
// expect: 20
// expect: 30
// expect: 40
// expect: 15
// expect: 100
// expect: 6
// expect: 99
// expect: 1
// expect: 5
// expect: 20
const Counter = struct { n: i64 };
const Holder = struct { values: []i64 };
const Cell = struct { n: i64 };
const Parent = struct { cell: Cell };

fn next(c: Counter) i64 {
    const n = c.n;
    c.n += 1;
    return n;
}

fn replace(h: Holder) i64 {
    h.values = []i64{100, 200};
    return 5;
}

fn replace_cell(p: Parent) i64 {
    p.cell = Cell{ .n = 99 };
    return 5;
}

fn fail() !i64 { return error.ChangeIndex; }

fn main() void {
    const c = Counter{ .n = 0 };
    var a = []i64{10, 20};
    a[next(c)] += 5;
    print(c.n);
    print(a[0]);
    print(a[1]);

    // Even a plain local index must be saved: the RHS changes its value.
    var i = 0;
    a = []i64{10, 20};
    a[i] += fail() catch { i = 1; 5 };
    print(i);
    print(a[0]);
    print(a[1]);

    // Evaluate the inner base first, and the final index exactly once.
    c.n = 0;
    var g = [][]i64{[]i64{10, 20}, []i64{30, 40}};
    g[next(c)][next(c) - 1] += 5;
    print(c.n);
    print(g[0][0]);
    print(g[0][1]);
    print(g[1][0]);
    print(g[1][1]);

    // Replacing a field-chain base must not redirect the write.
    const h = Holder{ .values = []i64{10, 20} };
    const original = h.values;
    h.values[0] += replace(h);
    print(original[0]);
    print(h.values[0]);

    const p = Parent{ .cell = Cell{ .n = 1 } };
    const cell = p.cell;
    p.cell.n += replace_cell(p);
    print(cell.n);
    print(p.cell.n);

    // Plain assignments use the same target-before-value evaluation order.
    i = 0;
    a = []i64{10, 20};
    a[i] = fail() catch { i = 1; 5 };
    print(i);
    print(a[0]);
    print(a[1]);
}
