// A list bounds-checks against its count, not against its backing array's
// capacity -- indexing the array directly would hand back a spare slot. The
// message is the one `a[i]` gives, which is why the prelude exposes
// `panic_index` at all.
// expect: 3
// panic: index 5 out of bounds (len 3)
const list = @import("std/list");

fn main() i64 {
    var xs = list.new();
    list.push(xs, 1);
    list.push(xs, 2);
    list.push(xs, 3);
    print_int(list.len(xs));
    print_int(list.get(xs, 5));
    return 0;
}
