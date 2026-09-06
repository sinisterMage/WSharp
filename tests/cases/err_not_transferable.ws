// error: cannot be sent to another worker
// error: because `fn(i64) i64` cannot
const Holder = struct { f: fn(i64) i64 };
fn twice(x: i64) i64 { return x * 2; }
fn main() i64 {
    const fs = gc_transfer([]fn(i64) i64{ twice });
    const hs = gc_transfer([]Holder{ Holder{ .f = twice } });
    return 0;
}
