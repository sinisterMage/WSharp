# The adversarial harness

`cargo test --workspace` asks whether the suite passes. Everything here asks a
different question: **does anything disagree with anything else, and does
anything break when it is left running.** A green suite does not answer either,
and v1.0 is supposed to be earned by sustained use rather than by a demo.

Nothing here needs `cargo`. Every script drives the built compiler as a
subprocess, exactly as a user would, which is also the only seam where the
binary, the runtime archive and `cc` meet.

```sh
# build first -- `cargo test` alone does not build `crates/wsharp-start`, so the
# AOT half links whatever runtime archive an earlier build left behind
nix-shell --run "cargo build --workspace"
```

| script | question | typical cost |
|---|---|---|
| `selftest.sh` | does the harness report what actually happened? | seconds, **no compiler needed** |
| `conform-selftest.sh` | can the conformance gate tell a rotted diagnostic from an intact one? | seconds, **no compiler needed** |
| `parity.sh` | do the three execution modes agree, per case? | 1–3 h for the corpus on one core |
| `conform.sh` | does every refusal still print the same diagnostic, span and help line? | seconds for the 64 rejection cases |
| `fuzz.pl` | does arbitrary input crash or hang the front end or a stdlib parser? | bounded by `--max-seconds` |
| `flake-rate.sh` | how often does one case do something different, and how long does it take when it does? | attempts × the case |
| `gc-pauses.sh` | how long does the collector stop the program for? | ~10 min for `gc_*` at 5 repeats |
| `soak.sh` | does a long-running process creep? | as long as you give it |
| `nightly.sh` | all of the above, once, into one report | hours |
| `compare.sh` | what moved since the previous run? | seconds |

Reports land under `target/harness/`, one directory per invocation, so two runs
can always be compared. Override with `WSHARP_HARNESS_REPORTS`.

Two dependencies past a shell and perl, both resolved in `lib.sh` and both
refused up front rather than per case:

- **`timeout(1)`**, which is coreutils and is *not in base macOS* — two of the
  four release triples. `gtimeout` is accepted, which is what Homebrew's
  coreutils installs. Without one, the harness exits 2 instead of running the
  corpus unbounded and turning a hang into a runner that dies hours later with
  no report.
- **the compiler's name**, which is `wsharp.exe` on Windows. Looking only for
  `wsharp` reported "no compiler" on the one triple whose parity run is hardest
  to reproduce by hand.

## Reading a report

Every report names the commit, the platform with its libc version, the mode, the
duration, and the seed where there is one. That is not ceremony: a finding
without those cannot be handed to anybody, and `docs/defects.md` asks for all of
them.

Two things every report is careful about, because getting them wrong is how a
harness lies:

- **The commit is pinned at the start of a run**, and the commit *now* is printed
  beside it. A long run on a shared checkout can finish on a different SHA than
  it started, and naming the finishing one claims to have tested code that was
  never run.
- **The compiler under test is the binary, not the source tree.** If
  `target/debug/wsharp` is older than the last change to `crates/`, the report is
  about the older code. Check the two SHAs the report prints against each other
  before believing a result about a recent fix.

## `selftest.sh` — is the gate itself working?

```sh
tests/harness/selftest.sh
```

`parity.sh` is a gate, and the failure mode a gate has that a test does not is
passing for the wrong reason: a comparison that stopped detecting divergence
reports an empty divergence list, which is exactly what success looks like.

So `selftest.sh` builds a **stub compiler** — a shell script that answers from
directives in a case file — and runs the real `parity.sh` over a corpus whose
verdicts are known in advance: three modes agreeing, two disagreeing about
stdout, two disagreeing about exit status, one mode disagreeing with itself, and
one where the only difference is noise `normalise` is supposed to remove. Then
it asserts each verdict, the detail string that names the mode and the stream,
that the diff was kept, and that the exit status is non-zero when something
diverged and zero when nothing did.

It needs no cargo, no `cc` and no W# compiler, which is why it runs in the
ordinary CI job on every pull request rather than in the nightly. `scripts/soak-report-selftest.sh`
is the same idea for criterion 1's gate.

What it does **not** test: anything about W#. A green selftest says the harness
can tell a divergence from agreement, not that there are none.

`conform-selftest.sh` is the same argument for `conform.sh`, and runs beside it
on every pull request. Its stub compiler prints diagnostics dictated by the case
file, so a corpus can contain a diagnostic whose span has moved by four columns,
one whose `= help:` line has vanished, one `check` and `run` refuse differently,
and one with no snapshot at all — and the selftest asserts the verdict for each,
that the kept diff names both the old and the new column, that `--bless` then
verify round-trips, and that an unsnapshotted corpus exits **3** rather than 0.
Twenty-four assertions, no cargo.

## `conform.sh` — what the language refuses, and in what words

```sh
tests/harness/conform.sh              # verify against the committed snapshots
tests/harness/conform.sh --bless      # rewrite them from the compiler under test
```

`parity.sh` pins what W# *accepts*. This pins what it *refuses*: for every case
in `tests/cases` whose header carries `// error:`, the whole rendered diagnostic
compared byte for byte against `tests/conformance/expected/<case>.diag` —
message, `-->` location, quoted source line, caret columns, secondary labels and
help line — plus a `check`-versus-`run` differential on every one of them.

Three regressions the existing substring checks cannot see, and this one can: a
span that rots to `1:1`, a `= help:` line that disappears (nothing in
`tests/cases` expects one today, and `wsharp-sema` sets over forty), and `check`
accepting a program `run` refuses — which has happened, and is what
`err_unpinned_generic.ws` documents.

Verdicts are `PINNED`, `CHANGED`, `ACCEPTED`, `NOSPAN`, `CHECKRUN` and
`UNBLESSED`; exit is 0 for met, 1 for not met, 2 for could-not-run and **3 for
cannot-be-answered**, which is what an empty snapshot set gets rather than a
green run. The full description, the bless procedure and the suite's limits are
in `tests/conformance/README.md`.

## `parity.sh` — differential testing across the three modes

```sh
tests/harness/parity.sh                      # the whole corpus
tests/harness/parity.sh --only gc_           # a subset
tests/harness/parity.sh --timeout 300        # a loaded machine needs more
```

Runs every `tests/cases/*.ws` three ways -- `run`, `run --gc-stress`, and
`build` then execute -- and compares the modes **to each other** rather than to
the case's header. Two paths that must agree turn "is this right?" into a
mechanical check; the existing Rust suite holds each mode to the header, which
cannot catch two modes being right in different ways or a header that says
nothing about the thing that differs.

Divergence is reported per case, with both outputs kept under `diffs/`.

**A case that disagrees with itself is a different finding.** When a difference
shows up, each side is re-run twice before it is called a divergence; a mode that
cannot reproduce its own output is reported as `NONDETERMINISTIC` and classified
by hand -- timing, ordering, resource exhaustion, and uninitialised state have
different owners. Determinism is checked only where a difference appeared,
because establishing it up front costs twice as much of a run that already takes
hours.

Output that legitimately differs between two runs is removed before comparison:
the collector's statistics line, temporary paths, and hexadecimal addresses. That
list is in `lib.sh` and is deliberately short -- everything on it is something
this harness cannot check.

## `fuzz.pl` — mutation fuzzing with reduction

```sh
tests/harness/fuzz.pl --target check --seed 1 --iterations 500
tests/harness/fuzz.pl --target json  --seed 1 --max-seconds 600
tests/harness/fuzz.pl --target toml  --replay target/harness/.../input.toml
```

| target | what runs | seed corpus |
|---|---|---|
| `check` | `wsharp check` — parser and type checker | `tests/cases/*.ws`, `examples/*.ws` |
| `run` | `wsharp run` — adds lowering, codegen and the runtime | the same |
| `json` | `std/json.parse`, via `drivers/fuzz_json.ws` | `corpus/json/` |
| `toml` | `std/toml.parse`, via `drivers/fuzz_toml.ws` | `corpus/toml/` |

What counts as a finding: a Rust panic, a signal, no answer inside the timeout,
and -- for the stdlib parsers only -- a W# panic, because "a parser answers, it
does not raise" is the rule those modules are written to. A diagnostic is not a
finding; most runs produce one and that is the intended outcome.

Three properties are load-bearing:

- **Seeds replay.** The generator is a 31-bit LCG written out in the script
  rather than perl's `rand`, so a seed means the same inputs on every machine and
  every perl build. The one thing that moves an input under a fixed seed is the
  seed *corpus* changing, which is why each finding is written out as a file and
  not as a seed number.
- **Findings arrive reduced.** Each new cause is shrunk by delta debugging
  against its own signature before it is recorded, so what lands in the report is
  usually already the `tests/cases` entry that will guard the fix. The
  unreduced input is kept beside it.
- **One directory per cause, not per occurrence.** Findings are keyed by
  signature -- panic site and message, signal number, or `hang` -- with a count,
  so a mutation class that trips one assertion four hundred times is one finding.

A stdlib driver is built once per campaign rather than compiled per input.
Compiling `fuzz_json.ws` takes about 2.5 seconds and parsing a document takes
milliseconds, so recompiling per iteration spends the whole budget on the
compiler while claiming to fuzz the parser; building once took 60 inputs from
157 seconds to under 10.

## `gc-pauses.sh` — pause times, and the counts that say they mean anything

```sh
tests/harness/gc-pauses.sh --only gc_ --repeats 5
tests/harness/gc-pauses.sh --only gc_ --repeats 3 --stress
```

**What is measurable today.** `WSHARP_GC_STATS=1` reports three numbers about
pauses per process -- how many, the longest, the total -- and `gc::record_pause`
keeps exactly those three counters. There is no histogram and no per-pause log,
so **a p99 over the pauses inside one run cannot be computed from outside the
runtime**, however the numbers are rearranged afterwards. What this measures is
the distribution over *runs*: the longest pause per run, which is the tail metric
that matters, and the mean per run. The report says which, every time.

Pauses are wall-clock time on the mutator thread, so on a loaded or single-core
machine they include time the thread was not scheduled. The report prints the
core count and the load average for that reason: a long tail on a busy box is a
number to reproduce on a quiet one, not a defect.

**The counts are checked.** A root walk that finds nothing makes every root check
pass for the wrong reason, so any sample reporting zero collections, zero roots
or zero pauses is called out rather than counted as a quiet success.

## `soak.sh` — the long windows

```sh
tests/harness/soak.sh \
    --command "./target/debug/wsharp run tests/harness/drivers/soak_self.ws" \
    --duration 604800 --interval 60 --label self --probe "curl -s localhost:8080/health"
```

Samples resident size, open descriptor count, cumulative stdout lines
(throughput), and a probe's latency, into `counters.tsv` **as it goes** -- a
window cut short by the machine going away still leaves every sample it took.

**Read the second-half slope, not the whole-window one.** Warm-up is a step and
creep is a slope. `drivers/soak_self.ws` reaches its working set in eleven
seconds, going from 12 MiB to 87 MiB and then moving 600 KiB over the next
eighty; fitted over the whole window that is "+1.6 GiB per hour", which would be
a serious leak if it were true. The report prints both and says which to read.

`drivers/soak_self.ws` exists so the sampler can be tested before there is
anything real to soak. It allocates continuously and keeps a bounded live set, so
resident size that climbs anyway is the collector's.

### The soak procedure

A soak is a job, not a heartbeat. Start it, write down where its output lands,
and come back.

1. **Start the window.** Give it `--label` and a real `--duration` (a week is
   `604800`). Use `setsid nohup` so it outlives the shell:

   ```sh
   nohup setsid tests/harness/soak.sh --label foundryd-w1 \
       --command "<the driver>" --probe "<a request>" \
       --duration 604800 --interval 60 \
       --out target/harness/soak-foundryd-w1 >/dev/null 2>&1 &
   ```

2. **Record in the task**: the output directory, the command, the probe, the
   window's intended length, the commit, and the platform. A later heartbeat
   resumes from that, and nothing else.

3. **Check in without waiting.** `tail counters.tsv` says whether it is still
   sampling. Do not sit in a loop watching it.

4. **Close the window** by reading `report.md`, then compare against the previous
   one:

   ```sh
   tests/harness/compare.sh target/harness/soak-foundryd-w0 target/harness/soak-foundryd-w1
   ```

5. **Say what actually ran.** If the window did not elapse, the report says so
   and so should the summary: *what ran, and for how long*. A window that was cut
   short is not a clean window.

## `flake-rate.sh` — a rate, not an adjective

```sh
tests/harness/flake-rate.sh --case tests/cases/https_loopback.ws --attempts 40
tests/harness/flake-rate.sh --case tests/cases/gc_moving.ws --mode stress
```

`parity.sh` can say that a case disagreed with itself. It cannot say how often,
and "it is flaky" is not something an owner can act on. This runs one case many
times and records **every attempt's duration** beside its verdict, because that
is what separates the two explanations a single timeout has: a long tail on a
busy machine is slowness, and a second mode far from the first — an attempt
taking several times the p99 and then hitting the timeout — is a stall, which is
a defect whatever the load was.

The report gives the rate per exit status, the p50/p90/p99/max duration, and the
load average at each attempt. Exit 124 is the harness timeout, not the program.

## `nightly.sh` and the matrix

`nightly.sh` is the per-platform half: parity, then pauses, then a bounded fuzz
campaign per target, into one directory with one `report.md`. The seed is the
date, so one number replays the night.

The matrix is fanned out and collected by `.github/workflows/nightly.yml`. The
*script* does not know it is in CI on purpose — that would be a second place the
matrix is defined — but the split between the two workflows is a decision worth
writing down here as well as there:

| workflow | when | what |
|---|---|---|
| `ci.yml` | every push and pull request | the suite, the lints, and both selftests (`selftest.sh`, `conform-selftest.sh`) — seconds, no cargo |
| `nightly.yml` | 03:00 UTC daily, and on demand | `parity.sh` and `conform.sh` on all four release triples, `nightly.sh` on Linux |

`parity.sh` is one to three hours per platform, so putting it on every pull
request would make the median change wait three hours and spend a runner-day per
push. What *does* run on every pull request is the harness's own verdict logic,
because a change that broke it would otherwise survive until a nightly whose
divergence list was empty for the wrong reason.

The nightly job fails on any divergence, any nondeterministic case, and any fuzz
finding. Nothing is `continue-on-error`: a gate that reports amber is a gate
nobody reads.

## Filing what it finds

Findings go to GitHub issues on `sinisterMage/WSharp` through
`.github/ISSUE_TEMPLATE/defect.yml`; the process is `docs/defects.md` and the
severity scale is `RELEASE-CRITERIA-1.0.md`. The harness produces what the form
asks for: the reduced program, the commands and their actual output, the full
40-character commit SHA, the platform with its libc version, and which execution
modes were actually tried.

Two rules the harness follows rather than leaves to the filer:

- **Reduce before filing.** An unreduced fuzz crash costs the fixer more than it
  cost the harness.
- **One issue per cause**, with a count, rather than one per occurrence.

And one it cannot decide for itself: **any abort under `--gc-stress` is P1**, with
no argument about whether it could happen without stress. Rarity is not a
mitigation.

## What this harness does not cover

A suite's limits belong where somebody reading its green run will see them.

- **`parity.sh` compares the modes, not the language.** Three modes agreeing on
  the wrong answer is `AGREED`. Holding each mode to its case header is
  `crates/wsharp-cli/tests/cases.rs`, and that is a different question; neither
  suite subsumes the other.
- **The corpus is `tests/cases`.** Parity's coverage is exactly what somebody
  wrote a case for. A construct with no case is not tested by this harness in
  any mode, and nothing here measures which constructs those are — grammar
  coverage is not computed.
- **`normalise` is a list of things that cannot be checked.** The collector's
  statistics line, temporary paths and hexadecimal addresses are stripped before
  two outputs are compared, so a divergence that shows up *only* in one of those
  is invisible here by construction. The list is in `lib.sh` and is short for
  that reason.
- **Determinism is sampled, not established.** A mode is re-run twice only when
  a difference has already appeared. A case that is nondeterministic but happened
  to agree across the three modes it ran in is recorded as `AGREED`;
  `flake-rate.sh` is what answers the question properly, and it is run by hand
  against a named case rather than across the corpus.
- **Fuzzing is mutation-based, not grammar-aware.** `fuzz.pl` mutates a seed
  corpus with a dictionary. It reaches deep parser states by starting from real
  programs, and it will not construct a valid-but-unusual program the corpus does
  not already resemble. There is no generator that knows the grammar, and no
  coverage feedback to steer one.
- **The `json` and `toml` targets fuzz two parsers.** Every other `std/*` module
  is unfuzzed, and so is `ingot`.
- **Nothing here fuzzes across a worker boundary**, drives many workers, or
  constrains the heap size. Those modes ship, and this harness does not exercise
  them.
- **`gc-pauses.sh` measures the distribution over runs, not within one.**
  `WSHARP_GC_STATS=1` reports three counters per process and there is no
  per-pause log, so a p99 over the pauses inside a single run cannot be computed
  from outside the runtime at all (#18). The report says which distribution it is
  reporting, every time.
- **Timing numbers here are not published numbers.** Pause and duration figures
  are for triage — is this a stall or a busy box — and anything quoted as a
  result goes through Ridge's harness under Form B of the proof standard.
- **CI covers four triples and one older-glibc container.** A platform not in
  that matrix has no evidence, and a pass on one triple is evidence about one
  triple. 32-bit and non-x86-64/aarch64 targets are out of scope (#11).
- **`selftest.sh` tests the harness, not the language**, and a green selftest
  says only that the gate can still tell a divergence from agreement.
- **`conform.sh` has no snapshots committed yet**, so today it proves the
  comparison works rather than that any W# diagnostic is correct: it exits 3,
  "cannot answer", until a bless commit lands. It also adds no new rejection
  cases — it pins the 64 that exist — and does not cover warnings, `// panic:`
  text, `build`, or any multi-file diagnostic. `tests/conformance/README.md` is
  the full list.
