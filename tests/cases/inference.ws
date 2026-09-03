// Nothing here is annotated; every type is inferred.
// expect: 7
// expect: true
// expect: 6.5
fn add(a, b) { return a + b; }
fn less(a, b) { return a < b; }
fn scale(x: f64) { return x * 2.0 + 0.5; }
fn main() i64 {
    print_int(add(3, 4));
    print_bool(less(1, 2));
    print_float(scale(3.0));
    return 0;
}
