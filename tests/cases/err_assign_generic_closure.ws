// A generic `fn` literal is a definition rather than a value, so there is no
// single code pointer to reassign.
// error: cannot assign to `g`, which is a generic `fn`
// error: definition rather than a value
fn main() i64 {
    const g = fn (x) { return x; };
    g = fn (y) { return y; };
    print_int(g(1));
    return 0;
}
