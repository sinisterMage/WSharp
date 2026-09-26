// expect: 16000
// expect: 16000
// expect: true
// A root set may name one object from several slots at once, and the pause
// that finishes marking has to move it for all of them.
//
// That pause moves everything the roots point at, precisely so that the
// concurrent copying which follows cannot move something the program is
// holding. The *first* slot it reaches forwards the object; every later slot
// reaches it with a forwarding address where the header was -- and answering
// those with the address they came in with left them pointing into a block
// about to be released. A write through such a slot went to the copy nobody
// would look at again and was lost. The load barrier never sees this: a local
// is not a field.
//
// `deeper` builds that root set -- one `Cursor` in two parameter slots of each
// of five frames, and `main`'s local besides -- and `step` then increments it
// through both of its own slots while allocating, so the increments and the
// collector's copying overlap. `gc_trace_start`/`gc_trace_finish` bracket the
// whole recursion, because the window a stale root can do harm in is the one
// between the two pauses, with the program running. Sixteen thousand
// increments must be sixteen thousand.
const Cursor = struct { index: i64, note: str };

fn churn(n: i64) i64 {
    // Garbage interleaved with what survives, so the surviving objects' blocks
    // end up mostly empty -- which is what makes them evacuation candidates
    // rather than merely recyclable. `gc_moving.ws` sets a trace up the same
    // way.
    var i = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Cursor{ .index = i, .note = "junk" };
        last = junk.index;
    }
    return last;
}

/// Step one counter through two slots naming the same object, allocating as it
/// goes so that the program keeps reaching safepoints and the collector keeps
/// moving things underneath it.
fn step(c: Cursor, alias: Cursor, n: i64) void {
    var i = 0;
    while (i < n) : (i += 1) {
        const junk = Cursor{ .index = i, .note = "junk" };
        if (junk.index < 0) { return; }
        c.index += 1;
        alias.index += 1;
    }
}

/// Recurse with `c` and `alias` -- one object -- in every frame's parameter
/// slots, and step it from each frame on the way back out.
fn deeper(c: Cursor, alias: Cursor, depth: i64) i64 {
    if (depth > 0) {
        const inner = deeper(c, alias, depth - 1);
        step(c, alias, 1000);
        return inner;
    }
    step(c, alias, 4000);
    return c.index;
}

fn main() i64 {
    const c = Cursor{ .index = 0, .note = "aliased" };
    churn(5000);
    gc_collect();
    gc_collect();

    gc_trace_start();
    deeper(c, c, 4);
    gc_trace_finish();

    // 4000 at the bottom and 1000 in each of the four frames above it, twice
    // over: once through each frame's own pair of slots.
    print(c.index);
    // Read again through the same local, after the trace has been drained.
    gc_collect();
    print(c.index);
    // Not a vacuous pass: a trace really did run.
    print(gc_traces() >= 1);
    return 0;
}
