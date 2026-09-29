// expect: 0
// expect: 0
// expect: 0
// panic: index 0 out of bounds (len 0)
//
// An element of `array.new(n)` whose type is itself an array is null until it
// is assigned, and a null array is empty: its length is 0, a `for` over it runs
// no times, and indexing it is an index out of bounds. #67 was this program
// segfaulting at the last line, because the inline bounds check read the
// length out of the header of an array that was not there.
const array = @import("std/array");

fn main() i64 {
    var g: [][]i64 = array.new(2);
    print(array.len(g[1]));
    var n = 0;
    for (g[1]) |x| { n += x + 1; }
    print(n);
    var total = 0;
    for (g) |row| { total += array.len(row); }
    print(total);
    print(g[1][0]);
    print("unreachable");
    return 0;
}
