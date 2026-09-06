// Workers, and the typed calls between them.
//
// A worker is an OS thread that owns its heap. A caller holds a handle -- an
// index, not a pointer, because a worker's objects are not the caller's to hold
// -- and every call copies its arguments there and its result back. That the
// call can fail is not an exceptional case worth a second mechanism, so it is
// `!T` like any other fallible thing, and `error.WorkerDied` is what it raises.
//
// A service is an ordinary module: `init` makes the state and a method is any
// function taking that state first. No new declaration form was needed, because
// W# has no mutable globals -- the state had to be an explicit value anyway.
// expect: 15
// expect: 22
// expect: 22
// expect: first
// expect: 100
// expect: 22
// expect: true
const counter = @import("./modules/counter.ws");

fn main() i64 {
    const w = @spawn(counter, 10, "first") catch return 1;
    print_int(w.add(5) catch -1);
    print_int(w.add(7) catch -1);

    // State persists between calls: it is the worker's, held for as long as
    // the worker lives, on the same runtime root list a deep copy uses.
    print_int(w.total() catch -1);
    // A string crosses as a copy, like any other reference.
    print(w.describe() catch "?");

    // Two workers running the same service are two heaps and two states.
    const other = @spawn(counter, 100, "second") catch return 1;
    print_int(other.total() catch -1);
    print_int(w.total() catch -1);

    @join(w) catch return 2;
    @join(other) catch return 2;

    // A call into a worker that has gone is the error the type promised.
    const gone = w.total() catch |e| if (e == error.WorkerDied) -9 else -1;
    print_bool(gone == -9);
    return 0;
}
