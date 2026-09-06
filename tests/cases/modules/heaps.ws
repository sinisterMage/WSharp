// A service that reports on its own heap, which is the point of the whole
// arrangement: a worker's objects are its own, and so are its collections.
const array = @import("std/array");

pub const State = struct { kept: []i64 };

pub fn init() State { return State{ .kept = []i64{} }; }

/// Allocate a lot, here. Nothing of it reaches the caller.
pub fn churn(s: State, n: i64) i64 {
    var i = 0;
    var last: []i64 = []i64{};
    while (i < n) : (i += 1) { last = array.new(4); }
    s.kept = last;
    return gc_live_objects();
}

/// One collection of *this* worker's heap.
pub fn collect(s: State) i64 {
    gc_collect();
    return gc_collections();
}

/// A whole mark trace, on this worker's heap and this worker's collector
/// thread. Synchronous: cycles are reclaimed by the time it returns.
///
/// This is what the per-worker split is for. The trace's three pauses run on
/// *this* mutator, and no other worker stops for them.
pub fn trace(s: State) i64 {
    var i = 0;
    while (i < 2000) : (i += 1) { s.kept = array.new(4); }
    gc_trace();
    return gc_traces();
}
