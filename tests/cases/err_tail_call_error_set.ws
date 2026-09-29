// error: which this function cannot
//
// #62, pinned as LIMITATIONS.md describes it: a tail `return last()` in a
// function whose error set is inferred makes that set *equal* last's, so an
// earlier `try first()` is refused as raising what the function cannot. The
// workaround, `return try last();`, is `tail_call_error_set_try.ws`.
fn first() !i64 { return error.First; }
fn last() !i64 { return error.Last; }
fn combine() !i64 {
    const value = try first();
    return last();
}
fn main() i64 {
    const result = combine() catch return 1;
    return 0;
}
