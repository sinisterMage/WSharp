// `and`/`or` must skip their right operand when the left already decides the
// answer -- and must still evaluate it when it does not.
// expect: false
// expect: true
// expect: called
// expect: true
// expect: called
// expect: false
fn bump(c: bool) bool { print("called"); return c; }
fn main() i64 {
    print_bool(false and bump(true));
    print_bool(true or bump(false));
    print_bool(true and bump(true));
    print_bool(false or bump(false));
    return 0;
}
