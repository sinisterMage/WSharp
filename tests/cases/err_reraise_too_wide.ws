// error: this raises `{Two}`, which this function cannot
// A caught error may be handed on, and is then held to the same rule `try` is:
// what leaves a function has to be something its set says it may raise. The
// span points at the value rather than at the call, which is the honest place
// -- `narrow` catches both and passes both on.
fn risky(n: i64) !{One, Two}i64 {
    if (n == 1) { return error.One; }
    return error.Two;
}

fn narrow(n: i64) !{One}i64 {
    return risky(n) catch |e| { return e; };
}

fn main() i64 { return narrow(1) catch 0; }
