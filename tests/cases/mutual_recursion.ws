// Two functions in one binding group, inferred together.
// expect: true
// expect: false
fn is_even(n) { if (n == 0) { return true; } return is_odd(n - 1); }
fn is_odd(n) { if (n == 0) { return false; } return is_even(n - 1); }
fn main() i64 {
    print_bool(is_even(10));
    print_bool(is_even(7));
    return 0;
}
