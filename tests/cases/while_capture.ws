// expect: 3
// expect: 2
// expect: 1
// expect: 30
// expect: 20
// expect: 10
// expect: done
// `while (opt) |v|` runs while the optional holds a value, binding it. The
// second loop moves the step into the continue expression, which is checked
// in the capture's scope so it can use `v`.
fn step(v: i64) ?i64 {
    if (v > 1) { return v - 1; }
    return null;
}
fn main() i64 {
    var cur: ?i64 = 3;
    while (cur) |v| {
        print_int(v);
        cur = step(v);
    }
    cur = 3;
    while (cur) |v| : (cur = step(v)) {
        print_int(v * 10);
    }
    print("done");
    return 0;
}
