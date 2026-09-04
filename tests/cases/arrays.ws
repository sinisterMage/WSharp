// expect: 10
// expect: 30
// expect: 60
// expect: 99
// expect: 100
// expect: b
// expect: 2
fn sum(xs: []i64) i64 {
    var total = 0;
    var i: i64 = 0;
    while (i < 3) : (i += 1) { total = total + xs[i]; }
    return total;
}
fn main() i64 {
    const xs = []i64{ 10, 20, 30 };
    print_int(xs[0]);
    print_int(xs[2]);
    print_int(sum(xs));

    var ys = []i64{ 1, 2, 3 };
    ys[1] = 99;
    print_int(ys[1]);
    // A compound assignment reads the element and writes it back.
    ys[1] += 1;
    print_int(ys[1]);

    const names = []str{ "a", "b" };
    print(names[1]);

    // Nested arrays, and an empty one, which is why the element type is
    // written rather than taken from the elements.
    const grid = [][]i64{ []i64{ 1, 2 }, []i64{} };
    print_int(grid[0][1]);
    return 0;
}
