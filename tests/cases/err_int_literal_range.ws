// error: does not fit in `u8`
// A literal takes the type it is used at, so the question "does this value fit"
// finally has an answer -- and is asked.
fn main() i64 {
    const x: u8 = 300;
    return 0;
}
