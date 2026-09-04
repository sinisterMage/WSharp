// The array half of the standard library. Everything that moves a reference
// from one object into another is written in W#, so the barriers apply by
// construction -- see `crates/wsharp-runtime/src/std/array.ws`.
// expect: 3
// expect: 5
// expect: 1 2 3 4 5
// expect: 2 3 4
// expect: 4
// expect: 7 7 7
// expect: 0
// expect: a b c
// expect: 3
// expect: a
// expect: c
const array = @import("std/array");
const str = @import("std/str");

fn show(xs: []i64) str {
    var out = "";
    var first = true;
    for (xs) |x| {
        if (first) { first = false; } else { out = str.concat(out, " "); }
        out = str.concat(out, str.from_int(x));
    }
    return out;
}

fn main() i64 {
    const a = []i64{ 1, 2, 3 };
    print_int(array.len(a));

    const both = array.concat(a, []i64{ 4, 5 });
    print_int(array.len(both));
    print(show(both));
    print(show(array.slice(both, 1, 4)));
    print_int(array.len(array.push(a, 9)));
    print(show(array.repeat(3, 7)));

    // Slicing past the end clamps rather than panicking.
    print_int(array.len(array.slice(a, 5, 9)));

    // Arrays of references go through exactly the same code.
    const words = str.split("a,b,c", ",");
    print(str.join(words, " "));
    print_int(array.len(words));
    print(words[0]);
    print(words[2]);
    return 0;
}
