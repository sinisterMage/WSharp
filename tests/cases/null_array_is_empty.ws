// expect: 0
// expect: 4
// expect: 0
// expect: 5
//
// The half of #67 that must keep working: a null element of `array.new(n)`
// behaves as an empty array wherever it is read -- the builtin length, the
// inline length a `for` uses, and `array.push`, which builds a new array from
// it -- and assigning one replaces the null outright.
const array = @import("std/array");

fn main() i64 {
    var rows: [][]i64 = array.new(3);
    print(array.len(rows[0]));
    rows[0] = array.push(rows[0], 5);
    rows[2] = []i64{ 1, 2, 3 };
    var seen = 0;
    for (rows) |row| { seen += array.len(row); }
    print(seen);
    print(array.len(rows[1]));
    print(rows[0][0]);
    return 0;
}
