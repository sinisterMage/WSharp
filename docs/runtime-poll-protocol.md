# Collector pause publication

A collector raises its worker's poll request before publishing MarkDone or
EvacDone with a release store. A mutator acquiring either phase can therefore
service it directly (allocation, trace_start or trace_finish) and clear the
request exactly once. A loop poll that arrives before phase publication only
observes the request; it leaves it pending until a pause or abandonment clears
it. Reversing either half loses the protocol: phase first can leave an orphan
request, and consuming on entry can lose an early request.

The change adds no roots or storage for managed pointers. Existing stack-map and
parked-worker root rules still apply. Tests force the publication boundary with
a test-only, thread-local callback; it runs after the phase lock is released.
The runtime regressions in tests/cases/runtime_poll_publication.rs are Rust
because W# cannot observe private per-worker request state. They are included by
the runtime unit suite. The accompanying gc_poll_trace_finish.ws follows the
ordinary language-case format and exercises generated callers.

## Lifecycle boundary

Workers are retained in the process-wide registry; they are not dropped when a
Rust thread exits. Normal RPC worker exit calls mark::quiesce, and runtime exit
quiesces registered workers. Abandoning a marking trace clears its request;
evacuation is driven to completion before quiescence returns.

Direct Rust runtime callers must finish their trace or quiesce their worker
before leaving it without a mutator. There is no thread-exit destructor that
automatically services an outstanding request. This repair does not add one or
claim arbitrary embedding/thread teardown is supported. In particular, it does
not establish a general proof of the process-wide flag aggregation across
multiple workers; that is separate from the per-worker publication invariant.

## Regression evidence (#32)

The test-only parent is `d701f79ffb5df0dbfea1c5e4678cfdeac49267f3`,
based on main `4e9249a6850f9721db13dbd61c63c57250eced11`. On that commit:

```sh
cargo test -p wsharp-runtime --lib --locked publication_tests -- --test-threads=1
```

All three tests fail deterministically (exit 101):

```text
request raised after its mark pause already ran
early poll consumed the pending pause
evacuation pause published before its request
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 76 filtered out
```

The same command passes after the fix. The complete runtime selection is:

```sh
cargo build --workspace --locked
cargo test -p wsharp-runtime --lib --locked
cargo clippy -p wsharp-runtime --all-targets --locked -- -D warnings
cargo fmt --all -- --check
```

Local result: 79 passed, zero failed; build, lint and formatting pass with
Rust 1.95.0 on Debian 13, glibc 2.41. The language case prints `42` in all
four modes (choose an output path for `CASE_BIN`):

```sh
target/debug/wsharp run tests/cases/gc_poll_trace_finish.ws
target/debug/wsharp run --gc-stress tests/cases/gc_poll_trace_finish.ws
target/debug/wsharp build tests/cases/gc_poll_trace_finish.ws -o "$CASE_BIN"
"$CASE_BIN"
WSHARP_GC_STRESS=1 "$CASE_BIN"
```

The Rust regression establishes the before/after failure; the W# case covers
generated callers and is not claimed to expose the race deterministically.
CI results for other platforms must be read from the fix PR's current head.
