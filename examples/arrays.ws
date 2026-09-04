// Arrays, `for` loops, and generic functions over them.
//
// An array is a heap object with its length in the header and its elements
// inline -- the same shape a string literal has. Indexing bounds-checks and
// panics on failure, like `.?` on a null optional: a failure the type system
// permits but the program must not perform.

const array = @import("std/array");
const str = @import("std/str");

// A type parameter is written when it has to be named, and inferred when not.
fn first[T](a: []T) T { return a[0]; }

fn sum(xs: []i64) i64 {
    var total = 0;
    for (xs) |x| { total = total + x; }
    return total;
}

// `for (xs) |x, i|` binds the index as well as the element.
fn show(xs: []i64) str {
    var out = "";
    for (xs) |x, i| {
        if (i > 0) { out = str.concat(out, ", "); }
        out = str.concat(out, str.from_int(x));
    }
    return out;
}

fn main() i64 {
    const xs = []i64{ 3, 1, 4, 1, 5 };
    print(show(xs));
    print_int(sum(xs));
    print_int(first(xs));
    print(first([]str{ "a", "b" }));

    // Every one of these returns a new array: the length lives in the header,
    // so there is no capacity to grow into. `std/list` is the type that has
    // one -- see examples/list.ws.
    print(show(array.push(xs, 9)));
    print(show(array.slice(xs, 1, 4)));
    print(show(array.concat(xs, []i64{ 9, 2 })));

    // `a[i]` is a place as well as a value.
    var ys = []i64{ 1, 2, 3 };
    ys[1] += 10;
    print(show(ys));
    return 0;
}
