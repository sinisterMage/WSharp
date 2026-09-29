// A worker that allocates in `init`, for `gc_many_workers.ws`.
//
// `@spawn` returns before `init` starts, and a method call is dispatched only
// once `init` has returned. So eight `@spawn`s in a row are eight `init`s
// running at the same time, on eight threads, into eight heaps -- where eight
// method calls in a row would be eight turns taken one after another, because
// a call waits for its answer.
const array = @import("std/array");

pub const State = struct { kept: []i64, live: i64 };

pub fn init(n: i64) State {
    var i = 0;
    var last: []i64 = []i64{};
    while (i < n) : (i += 1) { last = array.new(4); }
    return State{ .kept = last, .live = gc_live_objects() };
}

/// What this heap held when `init` finished. Called after `init` returns, so
/// asking it is also waiting for it.
pub fn churned(s: State) i64 { return s.live; }

pub fn collect(s: State) i64 {
    gc_collect();
    return gc_collections();
}
