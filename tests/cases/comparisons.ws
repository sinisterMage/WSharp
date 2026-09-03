// expect: true
// expect: false
// expect: true
// expect: false
// expect: true
// expect: false
fn main() i64 {
    print_bool(1 < 2);
    print_bool(2 < 1);
    print_bool(2 >= 2);
    print_bool(1 == 2);
    print_bool(1 != 2);
    print_bool(true and false);
    return 0;
}
