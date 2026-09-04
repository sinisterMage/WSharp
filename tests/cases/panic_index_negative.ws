// A negative index is caught by the same unsigned compare: as a `u64`, -1 is
// larger than any length.
// panic: index -1 out of bounds (len 3)
fn main() i64 {
    const xs = []i64{ 1, 2, 3 };
    var i = 0;
    i = i - 1;
    print_int(xs[i]);
    return 0;
}
