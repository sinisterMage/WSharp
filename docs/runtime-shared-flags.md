# Shared runtime flag publication

A design note for the two process-wide GC flag words -- `ws_gc_poll_flag` and
`ws_gc_evacuating_flag` -- and the per-worker contributions behind them
([#64](https://github.com/sinisterMage/WSharp/issues/64)). It covers how a
worker's request becomes visible to every thread. The ordering between a
worker's pause request and the phase that services it is a separate protocol
([PR #63](https://github.com/sinisterMage/WSharp/pull/63)); the collector needs
both to hold.

## Invariant and ordering

For each aggregate (polling or evacuation), whenever `SHARED_TRANSITION` is
unlocked, its count equals the number of true per-worker contributions and its
exported flag equals `count != 0`. All writers, including duplicate requests,
acquire the mutex before touching the per-worker bit. The lock covers the bit
swap, count update and Release flag store. A delayed clear therefore cannot
publish a value derived from an obsolete count after another writer returns.
Stronger atomic ordering alone would not make those separate operations one
transaction.

The mutex synchronizes writers; the AcqRel bit/count operations and Release
flag publication remain. Runtime readers use Acquire loads. Generated readers
keep their byte-load fast paths and exported symbols. The flag is not a
snapshot of intermediate count/bit values inside an unfinished transition.
The caller must finish request publication before publishing the phase that
allows servicing it (PR #63), and must finish arming evacuation before resuming
the mutator. The memory-order assumptions of generated loads are unchanged.

The lock protects both aggregates. It never acquires heap, mark-state, worker
registry or park locks, never allocates, and never invokes production callbacks.
Callers may already hold other runtime locks, so acquiring any of those locks
inside this critical section would introduce a lock-order hazard. Test-only
thread-local callbacks suspend publication for the deterministic scheduler.
The state contains no managed pointers: the collector's root set does not grow.
No runtime dependency is added.

## Why it matters, and what it does not cover

`emit_gc_poll` branches around `ws_gc_poll` when the aggregate byte is zero;
`gc::ws_gc_poll` also checks it. Without the lock, an interleaving of one
worker's clear with another's raise can leave the byte at zero while a request
is still set, suppressing that worker's poll until another transition restores
the byte. `emit_load_barrier` similarly bypasses resolution when the evacuation
aggregate is zero, and both flags go through the same helper, so a stale byte
there would skip a load barrier while objects move.

Workers remain process-lifetime allocations. The phase, parked-mutator and
shutdown/abandon protocols remain responsible for pairing contributions and
settling workers. The lock neither adds worker deregistration nor validates
all exit paths. It also does not turn duplicate per-worker request bits into
queues: callers still must obey the per-worker phase protocol. The mutex adds
writer contention; no performance claim is made.

## Test

The regression tests live in `tests/cases/runtime_shared_aggregate.rs`, which
`crates/wsharp-runtime/src/worker.rs` includes as the `aggregate_tests` unit-test
module:

- `clearing_worker_cannot_hide_new_request` -- one worker's clear is suspended
  after its count update while another raises; the flag must still read 1.
- `raising_worker_survives_other_worker_round_trip` -- a request overlapping
  another worker's request/clear round trip.

Both call the real `set_shared` with private atomics, so unrelated worker
activity cannot change the count, and a channel-driven scheduler enforces the
interleaving with no sleeps or retries. Each checks counts, flags, duplicate
transitions and final cleanup. Run them with:

```sh
nix-shell --run "cargo test -p wsharp-runtime --lib aggregate_tests"
```
