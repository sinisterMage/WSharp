// expect: 30
// expect: 0
// expect: 30
// expect: -1
// expect: 90
fn lookup(k: i64) ?i64 {
    if (k == 0) { return null; }
    return k * 10;
}
fn main() i64 {
    print_int(lookup(3) orelse 0);
    print_int(lookup(0) orelse 0);
    if (lookup(3)) |v| { print_int(v); } else { print_int(-1); }
    if (lookup(0)) |v| { print_int(v); } else { print_int(-1); }
    print_int(lookup(9).?);
    return 0;
}
