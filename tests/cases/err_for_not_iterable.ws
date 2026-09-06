// error: cannot be iterated
// error: no `iter`
const P = struct { x: i64 };
fn main() i64 {
    for (P{ .x = 1 }) |v| { print_int(v.x); }
    return 0;
}
