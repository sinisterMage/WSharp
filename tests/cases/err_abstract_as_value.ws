// error: `Number` is an abstract type
// error: cannot itself be one
// An abstract type classifies values for dispatch; it is not the type of one,
// so there is nothing for `n` to hold and nothing to return.
fn half(x: Number) Number {
    const n = Number;
    return x;
}

fn main() i64 { return 0; }
