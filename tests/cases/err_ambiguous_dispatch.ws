// error: is ambiguous
// error: this overload
// Neither overload is more specific than the other: the first wins on the
// first argument, the second on the second, and a call with two `Sub` values
// matches both. Julia's rule makes this a compile error rather than a coin
// flip, so adding an overload can never silently change which one runs.
const Base = struct { };
const Sub = struct : Base { };

fn pick(a: Sub, b: Base) i64 { return 1; }
fn pick(a: Base, b: Sub) i64 { return 2; }

fn call(a: Base, b: Base) i64 { return pick(a, b); }

fn main() i64 { return call(Sub, Sub); }
