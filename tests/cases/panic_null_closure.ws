// expect: 7
// panic: read through a null reference
//
// A closure is a reference too, and calling a null one reads its code pointer
// out of nothing.
const array = @import("std/array");

fn main() i64 {
    var fs: []fn(i64) i64 = array.new(2);
    fs[0] = fn (x: i64) i64 { return x + 4; };
    print(fs[0](3));
    print(fs[1](3));
    print("unreachable");
    return 0;
}
