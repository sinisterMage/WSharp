// expect: a,b,c
// expect: abc
// expect: one
// expect: -
// expect: ,,
// expect: 600000
// expect: abcabcabc
// expect: -
// `str.join` and `str.repeat` assemble their answer in one buffer.
//
// Both were `concat` in a loop, which copies the accumulator every time and so
// is quadratic in the total length: joining the 150,000 pieces below took 9.4
// seconds and now takes a few milliseconds. The last three lines are the ones
// that would have caught the rewrite going wrong -- a separator that is empty,
// parts that are empty, and a count of zero, each of which the old shape got
// right by accident rather than by arithmetic.
const text = @import("std/str");
const list = @import("std/list");

fn show(s: str) void {
    // A case cannot expect an empty line, so an empty answer prints as `-`.
    if (text.len(s) == 0) { print("-"); } else { print(s); }
    return;
}

fn main() i64 {
    show(text.join([]str{ "a", "b", "c" }, ","));
    show(text.join([]str{ "a", "b", "c" }, ""));
    show(text.join([]str{ "one" }, ","));
    show(text.join([]str{}, ","));
    show(text.join([]str{ "", "", "" }, ","));

    var parts: list.List[str] = list.with_capacity(200000);
    var i = 0;
    while (i < 150000) : (i += 1) { list.push(parts, "<li>"); }
    print_int(text.len(text.join(list.to_array(parts), "")));

    show(text.repeat("abc", 3));
    show(text.repeat("abc", 0));
    return 0;
}
