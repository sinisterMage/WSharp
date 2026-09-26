// Eight workers, eight heaps, eight collectors, all at once.
//
// `worker_heaps.ws` states the arrangement with one worker: a worker's objects
// are its own and so are its collections. This case states the part one worker
// cannot -- that the arrangement is *per* worker, and that N of them running
// concurrently do not become one shared thing by accident.
//
// Three workers was the corpus maximum before this, and none of those three
// allocated: `broker_workers.ws` sends messages, `worker_rpc.ws` makes calls.
// So nothing until now has had more than one heap collecting at a time. The
// pieces that only exist once per *process* are what this leans on:
//
//   * the two flag bytes generated code reads, which mean "*some* worker wants
//     a pause" and "*some* worker is moving". Eight mutators asking at once is
//     eight chances for the slow path to answer a request that is not its own
//     as though it were -- the check CLAUDE.md describes as "the slow path asks
//     the current worker whether the request is its own, and returns at once
//     when it is not";
//   * the mark parity, which each worker flips in its own initial pause;
//   * the space directory, which the load barrier asks about an arbitrary
//     address and which eight allocating heaps are writing into.
//
// Under `--gc-stress` every one of those 1600 allocations is a full collection
// with a stack walk, in eight threads that are not synchronised with each
// other, which is the state this case exists to put the runtime in. It is named
// `gc_*` so `the_collector_survives_stress_in_a_built_program` runs it as a
// built program under stress as well -- the pass where a stack-map
// serialisation mistake shows up.
//
// Counts rather than booleans throughout, so a wrong answer says how wrong.
//
// expect: 8
// expect: 8
// expect: 1
const heaps = @import("./modules/heaps.ws");

/// 1 when a worker answered with a live heap of its own, 0 when it raised or
/// found nothing. Summed, so the printed number names how many worked.
fn reported(n: i64) i64 {
    if (n > 0) { return 1; }
    return 0;
}

fn under(n: i64, limit: i64) i64 {
    if (n < limit) { return 1; }
    return 0;
}

fn main() i64 {
    const mine_before = gc_live_objects();

    const a = @spawn(heaps) catch return 1;
    const b = @spawn(heaps) catch return 1;
    const c = @spawn(heaps) catch return 1;
    const d = @spawn(heaps) catch return 1;
    const e = @spawn(heaps) catch return 1;
    const f = @spawn(heaps) catch return 1;
    const g = @spawn(heaps) catch return 1;
    const h = @spawn(heaps) catch return 1;

    // 200 each rather than `worker_heaps`'s 3000, because this runs eight times
    // over and the whole suite runs again under stress, where every allocation
    // is a collection. 1600 stressed allocations across eight unsynchronised
    // heaps is the interesting part; making each heap individually large is
    // `worker_heaps`'s job.
    var churned = 0;
    churned += reported(a.churn(200) catch -1);
    churned += reported(b.churn(200) catch -1);
    churned += reported(c.churn(200) catch -1);
    churned += reported(d.churn(200) catch -1);
    churned += reported(e.churn(200) catch -1);
    churned += reported(f.churn(200) catch -1);
    churned += reported(g.churn(200) catch -1);
    churned += reported(h.churn(200) catch -1);
    print_int(churned);

    // A collection of *that* worker's heap, eight times, each on its own
    // mutator thread. The counter each one reads is the program's, so this
    // says every worker's collector ran, not how many times.
    var collected = 0;
    collected += reported(a.collect() catch -1);
    collected += reported(b.collect() catch -1);
    collected += reported(c.collect() catch -1);
    collected += reported(d.collect() catch -1);
    collected += reported(e.collect() catch -1);
    collected += reported(f.collect() catch -1);
    collected += reported(g.collect() catch -1);
    collected += reported(h.collect() catch -1);
    print_int(collected);

    // The workers allocated 1600 objects between them and none of them is
    // here. The bound is deliberately loose -- eight `@spawn`s and eight
    // handles do land in this heap -- because the claim being tested is that
    // the *thousands* went somewhere else, not an exact accounting of what a
    // spawn costs.
    print_int(under(gc_live_objects() - mine_before, 400));

    @join(a) catch return 2;
    @join(b) catch return 2;
    @join(c) catch return 2;
    @join(d) catch return 2;
    @join(e) catch return 2;
    @join(f) catch return 2;
    @join(g) catch return 2;
    @join(h) catch return 2;
    return 0;
}
