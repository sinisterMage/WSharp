// `for` over something that is not an array.
//
// The subject's own module says how it is walked: `iter` turns it into an
// iterator and `next` produces `?T` until it produces null, which is the shape
// `while (c) |v|` already has. Resolved in the module that *declares the type*
// rather than the one the loop is written in, so iterating a list needs no
// import beyond the list itself.
// expect: 10
// expect: 20
// expect: 30
// expect: 60
// expect: 0 a
// expect: 1 b
// expect: 20
// expect: 3
// expect: 6
// expect: 9
// expect: 12
// expect: 7
const list = @import("std/list");
const str = @import("std/str");

// An iterable of this file's own: the protocol is a pair of functions, not a
// language feature, so anything can have one.
const Steps = struct { limit: i64 };
const Walk = struct { at: i64, limit: i64 };
fn iter(s: Steps) Walk { return Walk{ .at = 0, .limit = s.limit }; }
fn next(w: Walk) ?i64 {
    if (w.at >= w.limit) { return null; }
    w.at = w.at + 3;
    return w.at;
}

fn main() i64 {
    var xs = list.new();
    list.push(xs, 10);
    list.push(xs, 20);
    list.push(xs, 30);

    var total = 0;
    for (xs) |v| {
        print_int(v);
        total += v;
    }
    print_int(total);

    // The index is counted rather than read out of the subject, because an
    // iterator need not have one.
    var names = list.from([]str{ "a", "b" });
    for (names) |name, i| { print(str.concat(str.from_int(i), str.concat(" ", name))); }

    // `break` and `continue` are `while`'s, because this is a `while`.
    for (xs) |v| {
        if (v < 20) { continue; }
        print_int(v);
        break;
    }

    for (Steps{ .limit = 10 }) |v| { print_int(v); }

    // An array still takes the indexed path, unchanged.
    var sum = 0;
    for ([]i64{ 3, 4 }) |v| { sum += v; }
    print_int(sum);
    return 0;
}
