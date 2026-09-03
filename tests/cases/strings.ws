// expect: hello
// expect: with a	tab
// expect: hello
const GREETING: str = "hello";
fn main() i64 {
    print(GREETING);
    print("with a\ttab");
    print(GREETING);
    return 0;
}
