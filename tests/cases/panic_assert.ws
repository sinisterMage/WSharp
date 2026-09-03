// expect: checking
// panic: assertion failed
fn main() i64 {
    print("checking");
    assert(1 + 1 == 3);
    print("unreachable");
    return 0;
}
