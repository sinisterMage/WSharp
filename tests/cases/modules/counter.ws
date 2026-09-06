// A service: a module with an `init` that makes its state, and functions
// taking that state as their first parameter. No new declaration form -- W#
// has no mutable globals, so a worker's state had to be an explicit value
// passed in and out, and once it is, the functions that take it are exactly
// the things the worker can be asked to do.
pub const State = struct { total: i64, label: str };

pub fn init(start: i64, label: str) State {
    return State{ .total = start, .label = label };
}

pub fn add(s: State, n: i64) i64 {
    s.total = s.total + n;
    return s.total;
}

pub fn total(s: State) i64 { return s.total; }

pub fn describe(s: State) str { return s.label; }

// Not a method: it does not take the state, so no caller can reach it through
// a handle.
pub fn helper() i64 { return 0; }
