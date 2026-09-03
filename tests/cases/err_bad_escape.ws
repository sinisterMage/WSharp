// error: unknown escape sequence
// A lexer error, end to end: it must reach the user like any other.
fn main() i64 {
    print("tab\q");
    return 0;
}
