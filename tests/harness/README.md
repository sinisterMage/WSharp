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
| `parity.sh` | do the three execution modes agree, per case? | 1–3 h for the corpus on one core |
| `fuzz.pl` | does arbitrary input crash or hang the front end or a stdlib parser? | bounded by `--max-seconds` |
| `gc-pauses.sh` | how long does the collector stop the program for? | ~10 min for `gc_*` at 5 repeats |
| `soak.sh` | does a long-running process creep? | as long as you give it |
| `nightly.sh` | all of the above, once, into one report | hours |
| `compare.sh` | what moved since the previous run? | seconds |

Reports land under `target/harness/`, one directory per invocation, so two runs
can always be compared. Override with `WSHARP_HARNESS_REPORTS`.

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

## `nightly.sh` and the matrix

`nightly.sh` is the per-platform half: parity, then pauses, then a bounded fuzz
campaign per target, into one directory with one `report.md`. The seed is the
date, so one number replays the night.

The matrix -- the four release platforms and the older-glibc target -- is five of
these, fanned out and collected by CI. That workflow is not here on purpose: a
script that knew it was running in CI would be a second place the matrix is
defined.

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
