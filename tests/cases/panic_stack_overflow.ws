// expect: start
// panic: stack overflow
//
// #48: a recursion with no base case used to die with Rust's "has overflowed
// its stack" and SIGABRT under `wsharp run`, and a bare SIGSEGV when built --
// no W# diagnostic either way. This is the fuzzer's reduction, the base case
// of `closures_recursive.ws` deleted.
fn main() i64 {
    print("start");
    const count = fn [T](a: []T, i: i64) i64 {
        return 1 + count(a, i + 1);
    };
    print(count([]i64{ 7, 8, 9 }, 0));
    return 0;
}
