// Two workers, two heaps.
//
// That is the whole reason the collector's state moved onto a worker: all
// three pauses run on the mutator, because only a mutator can walk its own
// stack, so one shared heap would need every mutator to stop before any pause.
// Per-worker heaps keep each worker's pauses to itself -- and the price is
// that no object is reachable from two of them, so everything sent is copied.
// expect: true
// expect: true
// expect: true
// expect: true
// expect: 7
// expect: true
const heaps = @import("./modules/heaps.ws");
const array = @import("std/array");

fn main() i64 {
    const mine_before = gc_live_objects();
    const w = @spawn(heaps) catch return 1;

    // Thousands of objects, all on the worker's heap.
    const theirs = w.churn(3000) catch -1;
    const mine_after = gc_live_objects();
    print_bool(theirs > 0);
    // This heap has not grown: the worker allocated into its own.
    print_bool(mine_after - mine_before < 100);

    // A collection there is a collection of *that* heap -- but the counters
    // are the program's, not one worker's: a program's collector is what all
    // of its workers' collectors did, which is also what the exit report says.
    print_bool((w.collect() catch -1) >= 1);
    print_bool(gc_collections() >= 1);

    // What crosses is copied, so the caller can hold it without holding
    // anything of the worker's.
    const copied = w.churn(1) catch -1;
    var sum = 0;
    for ([]i64{ 3, 4 }) |v| { sum += v; }
    print_int(sum);
    print_bool(copied > 0);

    @join(w) catch return 2;
    return 0;
}
