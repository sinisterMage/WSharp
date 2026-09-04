// expect: 2
// panic: index 5 out of bounds (len 3)
fn main() i64 {
    const xs = []i64{ 1, 2, 3 };
    print_int(xs[1]);
    print_int(xs[5]);
    print("unreachable");
    return 0;
}
