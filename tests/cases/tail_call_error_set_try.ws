// expect: First
// expect: Last
//
// The workaround LIMITATIONS.md gives for #62: `return try last();` widens the
// function's inferred error set by `last`'s, as `try` does anywhere else, where
// a bare `return last();` would make the two sets equal and refuse the `try`
// before it. Both errors reach the caller.
fn first(fail: bool) !i64 {
    if (fail) { return error.First; }
    return 1;
}
fn last() !i64 { return error.Last; }
fn combine(fail_first: bool) !i64 {
    const value = try first(fail_first);
    return try last();
}
fn name(fail_first: bool) str {
    const v = combine(fail_first) catch |e| {
        if (e == error.First) { return "First"; }
        if (e == error.Last) { return "Last"; }
        return "other";
    };
    return "none";
}
fn main() i64 {
    print(name(true));
    print(name(false));
    return 0;
}
