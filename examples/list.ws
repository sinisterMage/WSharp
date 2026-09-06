// A growable array.
//
//   wsharp run examples/list.ws
//
// `[]T` is fixed-length: its count lives in the object header, and there is no
// capacity beside it, so `array.push` allocates a whole new array every call.
// `list.List[T]` is the second object that length needs -- a backing array
// whose header length is the *capacity*, and a count of how much of it is in
// use. Pushing writes into the spare tail; only a full list reallocates, and
// it doubles when it does, so a run of pushes is amortised constant time.
const list = @import("std/list");
const str = @import("std/str");

/// Collatz: how many steps `n` takes to reach 1, and the path it took.
fn path(n: i64) list.List[i64] {
    var steps: list.List[i64] = list.new();
    var v = n;
    list.push(steps, v);
    while (v != 1) {
        if (v % 2 == 0) { v = v / 2; } else { v = 3 * v + 1; }
        list.push(steps, v);
    }
    return steps;
}

fn show(xs: list.List[i64]) str {
    var out = "";
    // A list is walked directly: `for` over anything but an array calls `iter`
    // and `next` from the module that declares the type, so `std/list` says
    // how a list is iterated without the type checker knowing it exists.
    for (xs) |v, i| {
        if (i > 0) { out = str.concat(out, " -> "); }
        out = str.concat(out, str.from_int(v));
    }
    return out;
}

fn main() i64 {
    const p = path(7);
    print(show(p));
    print_int(list.len(p));

    // The list grew from nothing, so its capacity is the first power of two
    // past its length -- the spare tail is what makes the next push free.
    print_int(list.capacity(p));

    // Lists of references work the same way; a growth copies them through the
    // write and load barriers, which is why `std/list` is written in W#.
    var words: list.List[str] = list.new();
    list.extend(words, str.split("the quick brown fox", " "));
    list.push(words, "jumps");
    print(str.join(list.to_array(words), ","));

    // `pop` and `remove` hand a value back; both panic rather than returning
    // `?T`, for the reason `a[i]` does.
    print(list.pop(words));
    print(list.remove(words, 0));
    print_int(list.len(words));
    return 0;
}
