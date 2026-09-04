// The growable array. A `List[T]` is the second object an array's length
// needs: `items` is the backing array, whose header length is the capacity,
// and `count` is how much of it is in use. Growth is what this exercises --
// ten pushes into a list that started with no array at all reallocate three
// times, and every reallocation copies references through the barriers.
// expect: 10
// expect: 16
// expect: 0 1 4 9 16 25 36 49 64 81
// expect: 81
// expect: 9
// expect: 99 0 1 4 9 16 25 36 49 64 -1
// expect: 0
// expect: 99 1 4 9 16 25 36 49 64 -1
// expect: 7
// expect: a-b-c-d
// expect: 4
// expect: 4 5 6
// expect: 32
// expect: 0
// expect: 0
// expect: true
const list = @import("std/list");
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
    // `list.new()` needs no annotation: a local binding is monomorphic, so
    // the first `push` is what pins the element type.
    var xs = list.new();
    var i = 0;
    while (i < 10) : (i += 1) { list.push(xs, i * i); }
    print_int(list.len(xs));
    print_int(list.capacity(xs));
    print(show(list.to_array(xs)));

    print_int(list.pop(xs));
    print_int(list.len(xs));

    // Inserting at the count appends; everything from `i` on shifts along.
    list.insert(xs, 0, 99);
    list.insert(xs, list.len(xs), -1);
    print(show(list.to_array(xs)));
    print_int(list.remove(xs, 1));
    print(show(list.to_array(xs)));

    list.set(xs, 0, 7);
    print_int(list.get(xs, 0));

    // A list of references goes through exactly the same code, and this is
    // the one that would break if a growth copied a reference by hand.
    var ws: list.List[str] = list.new();
    list.extend(ws, str.split("a,b,c", ","));
    list.push(ws, "d");
    print(str.join(list.to_array(ws), "-"));
    print_int(list.len(ws));

    const ys = list.from([]i64{ 4, 5, 6 });
    print(show(list.to_array(ys)));

    var zs = list.with_capacity(32);
    list.push(zs, 1);
    print_int(list.capacity(zs));

    // `clear` drops the backing array too, so nothing it held stays alive.
    list.clear(xs);
    print_int(list.len(xs));
    print_int(list.capacity(xs));

    // An empty list is still a list.
    var empty: list.List[i64] = list.new();
    print_bool(list.len(empty) == 0 and list.capacity(empty) == 0);
    return 0;
}
