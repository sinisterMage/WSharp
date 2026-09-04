// Type parameters named on the declaration, so a parameter's type can be
// written rather than only inferred.
// expect: 42
// expect: true
// expect: hello
// expect: 7
// expect: 3
fn id[T](x: T) T { return x; }
fn first[A, B](a: A, b: B) A { return a; }
fn head[T](xs: []T) T { return xs[0]; }
fn main() i64 {
    print_int(id(42));
    print_bool(id(true));
    print(id("hello"));
    print_int(first(7, "ignored"));
    print_int(head([]i64{ 3, 4, 5 }));
    return 0;
}
