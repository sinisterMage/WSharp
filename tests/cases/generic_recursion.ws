// A recursive generic function. The call to `pick` inside `pick` is made
// before its group is generalised, so it records no type arguments until
// inference fills in the group's own variables afterwards.
// expect: 42
// expect: hi
// expect: 6
fn pick[T](x: T, again: bool) T {
    if (again) { return pick(x, false); }
    return x;
}
fn count[T](xs: []T, i: i64) i64 {
    if (i >= 3) { return 0; }
    return 2 + count(xs, i + 1);
}
fn main() i64 {
    print_int(pick(42, true));
    print(pick("hi", true));
    print_int(count([]i64{ 1, 2, 3 }, 0));
    return 0;
}
