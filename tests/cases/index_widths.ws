// An array index may be any integer type.
//
// An index is an `i64` at the machine level, so a narrower one is widened
// where it is used -- signed or unsigned as its own type says. Conversions
// between numeric types are written and never inferred, because a silent
// widening is how a 32-bit hash quietly becomes a 64-bit one; this is not that.
// An index is not a value the program keeps, it is an argument to one operation
// whose type is fixed, and `i64(i)` at every subscript said nothing a reader
// did not already know.
// expect: 12
// expect: 13
// expect: 11
// expect: 99
// expect: 30
// expect: 4
const array = @import("std/array");

fn main() i64 {
    var a: []i64 = array.new(4);
    a[0] = 10;
    a[1] = 11;
    a[2] = 12;
    a[3] = 13;

    const i: u8 = 2;
    const j: i32 = 3;
    const k: u32 = 1;
    print_int(a[i]);
    print_int(a[j]);
    print_int(a[k]);

    // As a place, too: the expression form and the assignment form go through
    // the same one function.
    a[i] = 99;
    print_int(a[2]);

    // A `u8` counter walking a byte array, which is the shape that met this.
    var bytes: []u8 = array.new(4);
    var n: u8 = 0;
    while (n < 4) : (n += 1) {
        bytes[n] = n * 5;
    }
    var total: i64 = 0;
    var m: u8 = 0;
    while (m < 4) : (m += 1) {
        total += i64(bytes[m]);
    }
    print_int(total);

    // A literal index still needs nothing said about it.
    print_int(a[3] - a[1] + 2);
    return 0;
}
