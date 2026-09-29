# Shared runtime flag publication

Fix for [WSharp #64](https://github.com/sinisterMage/WSharp/issues/64).
This is separate from the request/phase publication repair in
[PR #63](https://github.com/sinisterMage/WSharp/pull/63); neither repair alone
proves the whole collector protocol sound.

## Invariant and ordering

For each aggregate (polling or evacuation), whenever `SHARED_TRANSITION` is
unlocked, its count equals the number of true per-worker contributions and its
exported flag equals `count != 0`. All writers, including duplicate requests,
acquire the mutex before touching the per-worker bit. The lock covers the bit
swap, count update and Release flag store. A delayed clear therefore cannot
publish a value derived from an obsolete count after another writer returns.
Stronger atomic ordering alone would not make those separate operations one
transaction.

The mutex synchronizes writers; existing AcqRel bit/count operations and Release
flag publication remain. Runtime readers use Acquire loads. Generated readers
retain their existing byte-load fast paths and exported symbols. The flag is
not a snapshot of intermediate count/bit values inside an unfinished transition.
The caller must finish request publication before publishing the phase that
allows servicing it (PR #63), and must finish arming evacuation before resuming
the mutator. This patch does not change generated-load memory-order assumptions.

The lock protects both aggregates. It never acquires heap, mark-state, worker
registry or park locks, never allocates, and never invokes production callbacks.
Callers may already hold other runtime locks, so acquiring any of those locks
inside this critical section would introduce a lock-order hazard. Test-only
thread-local callbacks suspend publication for the deterministic scheduler.
The state contains no managed pointers: the collector's root set does not grow.
No runtime dependency is added.

## Production impact and limits

`emit_gc_poll` branches around `ws_gc_poll` when the aggregate byte is zero;
`gc::ws_gc_poll` also checks it. The old interleaving can therefore suppress a
pending worker's poll until another transition restores the byte, even though
that worker's request is still set. `emit_load_barrier` similarly bypasses
resolution when the evacuation aggregate is zero. Because both use the same
helper, the stale publication violates the prerequisite for that fast path too.
These are source-level consequences of the demonstrated aggregate defect, not
an end-to-end demonstration of memory corruption or a GC safety failure.

Workers remain process-lifetime allocations. Existing phase, parked-mutator and
shutdown/abandon protocols remain responsible for pairing contributions and
settling workers. This repair neither adds worker deregistration nor validates
all exit paths. It also does not turn duplicate per-worker request bits into
queues: callers still must obey the per-worker phase protocol. The mutex adds
writer contention; measurement belongs to Ridge, and no performance claim is
made here. Release gate definitions are unchanged; this defect and its guard
must be assessed through the existing defect/stability gates.

## Deterministic proof

The regression lives in `tests/cases/runtime_shared_aggregate.rs` and is included
by the runtime unit-test module. It calls the actual `set_shared` implementation
with private atomics so unrelated worker activity cannot change the count.
The scheduler suspends A after its count update. B probes the serialization
boundary: without a held lock it completes its transition before A publishes;
with the lock held it acknowledges contention, then blocks in the real helper
until A completes. Channels enforce the order, with no sleeps, retries or
ignored tests. The second case covers a request overlapping another worker's
request/clear round trip. Both check counts, flags, duplicate transitions and
final cleanup. The existing shared-worker assertion remains unchanged.

Baseline main: `4e9249a6850f9721db13dbd61c63c57250eced11`.
The fix's parent, containing only the regression and test hook:
`d70fa36e878349dfdd7fb497b36efbcf508bdd70`.

```sh
cargo test -p wsharp-runtime --lib --locked aggregate_tests -- --test-threads=1
```

Before (Rust 1.95.0, Debian 13, glibc 2.41): exit 101, 1 passed, 1 failed:

```text
worker::aggregate_tests::clearing_worker_cannot_hide_new_request ... FAILED
assertion `left == right` failed: aggregate flag must reflect remaining worker contributions
  left: 0
 right: 1
```

After: exit 0, 2 passed, 0 failed. The failure occurs after both threads finish;
count is 1 and B's contribution is true, but the flag was 0.

Additional checks:

```sh
cargo test -p wsharp-runtime --lib --locked
cargo clippy -p wsharp-runtime --all-targets --locked -- -D warnings
cargo fmt --all --check
cargo build --workspace --locked
```
