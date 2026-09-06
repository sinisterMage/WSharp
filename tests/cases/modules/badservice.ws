// A service whose `init` wants something that cannot cross: a function value
// is a code pointer plus an environment belonging to the heap it was made in.
pub const State = struct { n: i64 };
pub fn init(f: fn(i64) i64) State { return State{ .n = f(1) }; }
