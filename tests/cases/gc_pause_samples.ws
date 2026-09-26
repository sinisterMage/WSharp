// expect: true
// expect: true
// expect: true
// expect: true
// expect: true
// Per-pause data exists, and a percentile can be computed from it.
//
// Before this case the collector kept a count, a total and a maximum per
// worker and nothing else (issue #18). Those three support a mean and a
// maximum; a median or a 99th percentile needs the individual samples, and
// they were gone by the time anything could read them. So a pause p99 was not
// a number this project could produce at all, while the README published a
// pause figure.
//
// What is asserted here is the one claim everything else rests on: **N
// recorded pauses produce N samples**. `gc_pauses()` is the counter that
// already existed; `gc_pause_samples()` is the histogram's total. They must
// agree exactly. Neither builtin is a safepoint, so no pause can happen
// between the two reads and the comparison is an assertion rather than a race.
//
// The percentiles are then checked for the property that makes them readable
// at all -- non-decreasing in the percentile asked for, and bounded by the
// maximum. The exact microsecond values are a property of the machine and are
// deliberately not expected here; `tests/harness/gc-pauses.sh` is what
// measures them, under Form B.
//
// The unit tests in `crates/wsharp-runtime/src/pause.rs` cover the bucket
// arithmetic and the N-in-N-out property directly. This case covers the half
// they cannot: that the samples survive a real trace on a real heap, through
// both execution modes the suite runs, and in a built program where the
// statistics travel through the emitted tables.
const Node = struct { value: i64, next: ?Node };

fn churn(n: i64) i64 {
    var head: ?Node = null;
    var i: i64 = 0;
    var total: i64 = 0;
    while (i < n) : (i += 1) {
        head = Node{ .value = i, .next = head };
        total = total + i;
    }
    return total;
}

fn main() i64 {
    // Enough traces to have run all three pauses several times over: the
    // initial one, the one that finishes marking, and -- because the churn
    // leaves sparsely occupied blocks behind -- the one that fixes references
    // after evacuating.
    var t: i64 = 0;
    while (t < 6) : (t += 1) {
        const sum = churn(2000);
        if (sum < 0) { return 1; }
        gc_trace();
    }

    // Read both before printing: `print` allocates, and an allocation is a
    // safepoint.
    const pauses = gc_pauses();
    const samples = gc_pause_samples();

    // The pauses happened at all, so the rest of this is about a non-empty
    // distribution rather than about zero.
    print(pauses > 0);
    // The claim: every pause left a sample behind.
    print(samples == pauses);

    const p50 = gc_pause_percentile_us(50);
    const p90 = gc_pause_percentile_us(90);
    const p99 = gc_pause_percentile_us(99);
    const max = gc_pause_percentile_us(100);

    print(p50 <= p90);
    print(p90 <= p99);
    print(p99 <= max);
    return 0;
}
