// expect: 5
// panic: unwrapped a null optional
fn lookup(k: i64) ?i64 {
    if (k == 0) { return null; }
    return k;
}
fn main() i64 {
    print_int(lookup(5).?);
    print_int(lookup(0).?);
    print("unreachable");
    return 0;
}
