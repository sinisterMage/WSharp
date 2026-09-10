// A worker whose `init` never returns.
//
// The shape of every acceptor: the loop *is* the worker. It never reaches its
// message loop, so it never sees the `Stop` the exit path sends -- which is
// exactly what `main` returning has to be able to end.
//
// It prints nothing, deliberately. `@spawn` hands the handle back before `init`
// starts, so whether this thread got as far as a `print` before `main` returned
// is a race, and a case cannot expect the answer.
pub const State = struct { ticks: i64 };

pub fn init(note: str) State {
    var i = 0;
    // Nothing is allocated, so `--gc-stress` has nothing to collect here; the
    // back-edge safepoint is what keeps the loop answerable to a pause anyway.
    while (true) : (i += 1) {}
    return State{ .ticks = i };
}

pub fn ticks(s: State) i64 { return s.ticks; }
