// expect: big
// expect: 1
fn main() i64 {
    const label = if (10 > 3) "big" else "small";
    print(label);
    print_int(if (false) 0 else 1);
    return 0;
}
