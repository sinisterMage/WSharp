// `var f = fn ...` is one storage location holding one function value, so it
// names one type. Only a `const` bound to a `fn` literal is a definition, and
// only a definition generalises -- this is the value restriction, and it is
// why the second call is a mismatch rather than a second instantiation.
// error: type mismatch
// error: expected `i64`
fn main() i64 {
    var f = fn (x) { return x; };
    print_int(f(1));
    print(f("two"));
    return 0;
}
