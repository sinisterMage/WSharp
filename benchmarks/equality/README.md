# Equality depth-bound comparison

From a clean checkout on Linux with Python 3, Git, Rust 1.95.0, a native C linker,
crate access and enough disk space, run:

```sh
python3 benchmarks/equality/run.py --output /absolute/path/to/new-results
```

The command clones and builds exact historical arms `f2f2b40` and `de3c792` --
consecutive commits on `main` that differ by exactly the depth bound (#29) --
with `cargo +1.95.0 build --release --locked`. To reuse clean, detached checkouts
named by full SHA, add `--arms-root /absolute/path/to/arms`. Both release builds
are still verified. Source and output paths must be outside the harness checkout
for a clean provenance record. Every attempt must use a new output directory;
retain failures as well as successful attempts. `--smoke` validates execution and
checksums only, with too few samples for performance conclusions.

The generated microbenchmark compares independent depth-16 objects two million
times, alternating equal and unequal leaf values. The synthetic whole-program
case reconciles 20,000 depth-3 records with allocation and integer transformation
work. It is a bounded synthetic application model, not evidence for arbitrary
production applications. Inputs and loop counts are fixed, shared by both arms,
and saved with SHA-256 hashes. Every process must produce an independently
calculated checksum. No changes to either compiler or runtime are made.

Both workloads run as AOT executables (compilation excluded) and fresh-process
JIT invocations (compilation included). Three untimed runs per combination warm
filesystem pages; every measured sample remains a new process. There is no claim
of in-process JIT steady state or isolated equality-operation latency. AOT
measurements include process startup, runtime setup, output and teardown.

A 60-second idle activity gate precedes sampling: every interval must have CPU
busy <5%, I/O wait <1%, steal <1%. Failure aborts and retains evidence. During
measurement aggregate CPU tick deltas are retained per process; do not interpret
workload CPU busy as idle contention. Virtualization can introduce noise after
the gate. No CPU affinity, governor changes or exclusive-host guarantee is made.

Noise is measured first by randomly interleaving two labels of the **same
baseline binary**, four blocks of ten samples per label. Then ten blocks of ten
samples per arm are interleaved with the same deterministic RNG seed. Raw
warmups and all measured rows are retained in JSONL. Summary JSON reports median,
nearest-rank p99, sample variance, per-block medians, variance across block
medians, paired percentage changes, and peak RSS from per-child `wait4`. The
maximum absolute A/A block-median change is a descriptive noise envelope, **not**
a confidence interval, significance test, or proof of equivalence. Assess sign
consistency and noise before interpreting any difference. With 100 samples per
arm, p99 is only the second largest observation; do not claim a stable tail.

The timer covers process creation, wait and redirection-file setup/close; reading
and verifying output and sampling /proc happen outside the timer. File logging
and /proc reads can perturb subsequent runs. The identical external instrument
applies to both arms, adds no runtime hooks, and is not enabled in normal release
binaries. Its absolute overhead is not independently calibrated; short-duration
or small-delta conclusions require a sensitivity experiment. RSS is peak process
resident memory, not bytes allocated. Historical arms do not expose cumulative
allocation bytes, so that quantity is explicitly not measurable here.

Keep the entire output directory as an artifact: metadata, exact command,
harness commit and dirty state, binary/lock/source hashes, toolchain and machine
info, build logs, generated workloads, quiet samples, raw timings, and summary.
Numbers require review before publication. This harness makes no performance
claim and does not hold the correctness fix.
