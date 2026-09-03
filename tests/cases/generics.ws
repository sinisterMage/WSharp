// One generic function, specialised at three types.
// expect: 42
// expect: true
// expect: hello
fn id(x) { return x; }
fn main() i64 {
    print_int(id(42));
    print_bool(id(true));
    print(id("hello"));
    return 0;
}
