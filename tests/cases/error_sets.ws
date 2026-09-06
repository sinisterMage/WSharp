// `!T` says which errors, not merely that there are some.
//
// The set rides as a second type argument, so `unify` needed no special case:
// two error unions unify by unifying their sets, and *widening* a smaller set
// into a larger one is `coerce`'s job -- exactly where widening a subtype into
// its supertype already lived. An unwritten set is inferred from what the body
// raises and propagates; a written one is checked against it.
// expect: 4
// expect: fine
// expect: negative
// expect: zero
// expect: 4
// expect: -2
// expect: missing
// expect: true
// expect: 3
// expect: -1
const io = @import("std/io");

fn risky(n: i64) !i64 {
    if (n < 0) { return error.Negative; }
    if (n == 0) { return error.Zero; }
    return n;
}

// Written down, and wider than what is returned into it -- which is the
// coercion, not a mismatch.
fn wide(n: i64) !{Negative, Zero, Other}i64 { return risky(n); }

// `try` propagates, so this one's inferred set covers the callee's.
fn chain(n: i64) !i64 { return try risky(n) + 1; }

// A function that raises nothing has the empty set, which prints as the bare
// `!i64` every signature was before sets existed.
fn safe(n: i64) !i64 { return n; }

// Generic over what its argument raises: the parameter's set is a variable,
// quantified with the rest of the signature and instantiated per call.
fn twice(f: fn(i64) !i64, x: i64) !i64 { return try f(try f(x)); }

fn which(n: i64) str {
    const v = risky(n) catch |e| {
        if (e == error.Negative) { return "negative"; }
        if (e == error.Zero) { return "zero"; }
        return "other";
    };
    print_int(v);
    return "fine";
}

fn main() i64 {
    print(which(4));
    print(which(-1));
    print(which(0));
    print_int(chain(3) catch -1);
    print_int(wide(-1) catch -2);

    // A builtin's set comes from its row in the table: it is compiled long
    // before the program that catches it, so nothing else could work it out.
    print(
        io.read_file("/tmp/wsharp_case_definitely_absent")
            catch |e| if (e == error.NotFound) "missing" else "other",
    );
    print_bool(safe(7) catch 0 == 7);

    // A set a caller decides. `twice`'s own is a variable its scheme
    // quantifies, so passing something that raises `Negative` is what makes
    // this call raise it -- and passing something infallible would not.
    print_int(twice(risky, 3) catch -1);
    print_int(twice(risky, -3) catch -1);
    return 0;
}
