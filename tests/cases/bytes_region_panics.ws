// An `at`/`n` region in `std/bytes` is bounds-checked, not clamped.
//
// The other half of the convention the module now states. A `from`/`to` range
// is clamped, which `bytes_ops.ws` checks; an offset and a count is reached
// through `b[i]` like any other index, so a count that goes past the end is the
// ordinary index panic and not a short write. Which matters because a short
// write is silent: a cipher whose last block was quietly two bytes shorter than
// it asked for produces a wrong answer rather than a report.
//
// `fill` is the one that stands for the family -- `xor` and `put_bytes` reach
// past their buffers the same way, through the same check, and a case can only
// expect one panic because the first ends the program.
//
// expect: two bytes
// panic: index 2 out of bounds (len 2)
const bytes = @import("std/bytes");

fn main() i64 {
    const b = bytes.new(2);

    // In bounds: the whole buffer, named as an offset and a count.
    bytes.fill(b, 0, 2, 0xff);
    if (b[0] != 0xff or b[1] != 0xff) { return 1; }
    print("two bytes");

    // One byte past it. Not a shorter fill.
    bytes.fill(b, 0, 3, 0x00);
    print("not reached");
    return 0;
}
