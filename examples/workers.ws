// Workers: OS threads that own their heaps, talking by typed calls.
//
//   wsharp run examples/workers.ws
//
// A service is an ordinary module. `init` makes the state; a method is any
// function taking that state as its first parameter. No new declaration form
// was needed for it, because W# has no mutable globals -- a worker's state had
// to be an explicit value passed in and out, and once it is, the functions
// that take it are exactly the things the worker can be asked to do.
const counter = @import("./modules/counter.ws");
const str = @import("std/str");

fn main() i64 {
    // Each of these is a thread with a heap of its own. Nothing either of them
    // allocates is ever reachable from the other, which is what lets their
    // collectors pause independently -- and what makes everything sent a copy.
    const orders = @spawn(counter, 0, "orders") catch return 1;
    const errors = @spawn(counter, 0, "errors") catch return 1;

    var i = 0;
    while (i < 5) : (i += 1) {
        // A call returns `!T` because a worker can die, and that is not an
        // exceptional case worth a second mechanism.
        const total = orders.add(i) catch |e| if (e == error.WorkerDied) -1 else -2;
        if (i % 2 == 0) {
            const failed = errors.add(1) catch -1;
            print(str.concat("failures so far: ", str.from_int(failed)));
        }
        print(str.concat(
            str.concat(orders.describe() catch "?", " total: "),
            str.from_int(total),
        ));
    }

    @join(orders) catch return 2;
    @join(errors) catch return 2;
    return 0;
}
