// Type inference.
//
// Not a single type is written down here. Hindley-Milner works all of them out,
// and `wsharp check examples/inference.ws --emit=types` will show you what it
// decided:
//
//     add:    fn(i64, i64) i64
//     scale:  fn(f64) f64
//     id:     fn(T) T
//     first:  fn(T, U) T
//     main:   fn() i64

fn add(a, b) { return a + b; }

// `x * 2.0` forces f64, and that flows out through the return type.
fn scale(x) { return x * 2.0; }

// Nothing constrains `x`, so `id` is generic. Each use below compiles to its
// own specialised copy of the function.
fn id(x) { return x; }

fn first(a, b) { return a; }

fn main() i64 {
    print_int(add(20, 22));
    print_float(scale(1.5));

    print_int(id(7));
    print_bool(id(true));
    print(id("generic"));

    print_int(first(1, "ignored"));
    return 0;
}
