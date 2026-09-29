# What v1.0 means

**The release gates for W# 1.0; amendments are listed at the end**, in
[Revision history](#revision-history). Where another document and this one
disagree about what 1.0 requires, this one is either right or gets amended here,
never worked around locally.

W# is at **0.2.3**. Every numbered item in [ROADMAP.md](ROADMAP.md) is marked
done, so the gap between here and a credible 1.0 is not features. It is
*sustained real use*, the defects that only sustained real use finds, and
evidence that anyone can re-run. This document is therefore a stability and
evidence bar rather than a feature checklist: it says what has to be true, how
each thing is measured, and what artefact closes it.

A criterion that needs a judgement call is not a criterion. Each of the eight
below is written so that a script, or a person reading a script's output, can
answer yes or no without arguing. Where a criterion still contains a judgement,
that judgement is named and recorded as a dated decision rather than left
implicit.

## How to read a criterion

Each one carries these fields:

| Field | Means |
|---|---|
| **Gate** | The yes/no question. Where a command answers it, the command is given. |
| **Evidence** | The artefact that closes it — a run, a file, a table. Not an assertion. |
| **Depends on** | Work elsewhere that the gate cannot pass without, where there is any. |

"Verified" in this document means a script ran and its output was recorded. A
thing checked by hand once is a thing that regresses silently, so a manual check
is not evidence; the script that encodes it is. What counts as evidence is
defined once, in [Part four: the proof standard](#part-four-the-proof-standard),
and every criterion points there.

## Where to start reading

| If you want | Read |
|---|---|
| A criterion's gate | [Part two](#part-two-the-eight-criteria), then [the issue map](#part-three-the-issue-map) |
| What you must produce to close it | [Part four: the proof standard](#part-four-the-proof-standard) |
| Whether a change is allowed during the freeze | [Part one](#part-one-definitions) |
| What 1.0 promises users | [Part five](#part-five-the-api-and-stability-freeze), then [COMPATIBILITY.md](COMPATIBILITY.md) |
| Which platforms count | [The platform matrix](#the-platform-matrix) |

---

# Part one: definitions

These three definitions are load-bearing. Criterion 2 is a gate on both of the
first two, and the defect intake process ([docs/defects.md](docs/defects.md))
cites the severity scale rather than restating it — there is one definition, in
this file, and everything else points here. If the intake and this document ever
disagree, this document is wrong and gets fixed; two definitions is the failure
mode being avoided.

## A breaking change

A change is **breaking** if a program that compiled and behaved correctly
against the previous tagged release would, after the change, either fail to
compile, behave observably differently, or need a source edit to keep either
property.

Concretely, each of these is breaking:

- Removing or renaming a `pub` name in `std/*` or `ingot/*`, or changing its
  signature, its parameter order, or its declared **error set**. An error set is
  part of a function's type here, so widening one is as breaking as narrowing
  one.
- Changing the meaning of existing syntax, or removing a syntactic form.
- Changing an inference rule so that a program accepted before is now rejected,
  **or so that an accepted program's inferred types change**. Accepting a
  program that was rejected before is additive, not breaking.
- Adding an overload that makes a call site which previously resolved
  unambiguously now ambiguous — or, worse, resolve to a different member. Adding
  an overload is not automatically additive in a language with multiple dispatch.
- Changing a type's place in the dispatch lattice, or a struct's field order.
- Changing the layout or content of any format something else reads: the three
  emitted tables (`MAGIC`/`VERSION` in `wsharp-runtime/src/aot.rs`), `ingot.env`,
  the lockfile, a store tree hash, `--emit=api` output, the release archive's
  internal layout, the artefact naming scheme, or the shape of the release feed
  sharpie reads.
- Changing a CLI verb's name, a flag's meaning, an exit status, or the columns of
  a TSV line, for `wsharp`, `ingot` or `sharpie`.

And each of these is **not** breaking:

- Adding a name, a module, a flag, or an overload that leaves every existing call
  site resolving exactly as it did.
- Fixing a P1. A program that depended on a miscompilation was never correct, and
  saying otherwise would make the severest class of defect unfixable.
- Diagnostic wording, performance, internal refactoring, tests, CI.

**Mechanism.** Every pull request carries exactly one of the labels
`change:breaking`, `change:additive`, `change:fix`, `change:none`. CI fails a
pull request carrying none or more than one. This is what makes "was there a
breaking change in the window" a query rather than a memory, and it must be in
place **before** day 0 — a window whose merges were not labelled cannot be
checked after the fact.

## The severity scale

Severity is decided by **what the defect does**, not by how annoying it is or how
hard it is to hit. Rarity is not a mitigation; a silently wrong answer that
happens once a month is still a silently wrong answer.

### P1 — blocks the tag, and resets the freeze clock

Any one of the following:

1. **A silently wrong answer.** A program the compiler accepts produces an
   incorrect result with no diagnostic: the wrong overload selected, wrong
   arithmetic, a field read from the wrong offset, a wrong comparison. The
   dispatched-call defect written up in item 13 of ROADMAP.md is the canonical
   instance.
2. **Memory unsafety.** The collector frees, moves or fails to trace a reachable
   object; generated code reads through a stale or unbarriered reference; a stack
   map describes a slot wrongly. **Any abort under `--gc-stress` is P1 by
   definition**, with no further argument about whether it could happen without
   stress.
3. **A crash with no W# diagnostic** — SIGSEGV, SIGILL, `0xC0000005`, an abort,
   or a hang with no progress — from a program that uses no FFI.
4. **Persistent-artefact corruption.** Anything the toolchain writes and later
   reads is left wrong or unreadable: a store entry, a lockfile, `ingot.env`, an
   installed toolchain directory.
5. **A broken install or upgrade path.** An install or upgrade that can leave a
   machine with no working toolchain; an interrupted operation that leaves a
   half-written one in place of a working previous one; any path that executes a
   downloaded artefact whose digest was not checked first.
6. **A broken integrity promise.** A published artefact whose bytes do not match
   its published digest, or a pinned toolchain that resolves to different bytes
   than it resolved to before. Pinning that does not pin is pinning that means
   nothing.
7. **A security defect in the trusted surface.** `std/tls`, `std/x509`, or the
   download path: accepting a certificate chain that must be refused, accepting a
   forged signature or AEAD tag, or transmitting key material in clear.
8. **A credential in a workflow file, a commit, or a published artefact.**

**Clause 3 carries no exemption for documented behaviour.** This was asked
directly on [#13](https://github.com/sinisterMage/WSharp/issues/13).
**Decision (2026-09-26): no.** A crash is a crash whether or not a
file says it will happen; documentation changes who is surprised, not what the
process does. A limitation may be *refused with a diagnostic* and documented —
that is a limitation. A limitation that aborts the process is a P1 and gets
bounded. The consequence for #13 is recorded in
[Part three](#part-three-the-issue-map).

### P2 — does not block the tag on its own; must be fixed or documented

- The compiler rejects a program it should accept, or reports the wrong cause.
- A stdlib function returns a wrong answer for inputs inside its documented
  domain but the wrongness is *reported* rather than silent (an error, a panic
  with a message).
- A platform arm fails where another passes, for a non-P1 reason.
- A performance regression of more than 2x on any case in `tests/cases`.
- A documented example that does not run, or a documented command that does not
  work as documented.

An open P2 at the tag must appear in the release notes' limitations list. An open
P2 that is neither fixed nor listed is a P1 against criterion 6, because it makes
the documentation wrong.

### P3 — cosmetic

Wording, formatting, ergonomics, a diagnostic that is correct but could be
kinder. Does not gate anything **except** where a gate names the issue
explicitly, which is how [#8](https://github.com/sinisterMage/WSharp/issues/8)
reaches gate 6d: a P3 that wastes a contributor's afternoon is still worth one
line in a CI job.

### Who decides

The filer proposes a severity; a maintainer confirms or changes it at triage,
within one working day. The `area:` labels in [docs/defects.md](docs/defects.md)
name the surface a report belongs to.

Disagreement about a severity goes to the repository owner and is settled within
one working day. **An unconfirmed report counts at the severity the filer
proposed** until it is triaged, so "no open P1" cannot be satisfied by leaving
reports untriaged.

## The freeze clock

The freeze is a **window of 28 consecutive days** ending at the tag, during which
no breaking change lands and no P1 is open.

- **Day 0** is the commit on `main` declared as the freeze start. The repository
  owner declares it, and records it in this file under "Freeze record" and in
  `.github/freeze-start`, which `scripts/freeze-check.sh` reads.
- The clock **resets to day 0** on either of: a merge to `main` labelled
  `change:breaking`, or the confirmation of a P1 in the compiler, runtime,
  stdlib, release pipeline, install scripts or sharpie. On a P1 the clock
  restarts when the fix **and its regression guard** are merged, not when the
  defect is diagnosed.
- The clock **does not reset** on: a P2 or P3 fix, a docs change, a test
  addition, a CI or workflow change, or a P1 confined to a driver program that is
  not itself shipped as an artefact. A driver-program P1 resets *that program's*
  soak counter under criterion 1 instead.
- Only the repository owner may grant an exception, only in writing on a GitHub
  issue, and every exception is recorded in the "Exception log" below with what
  changed, why it could not wait, and what the clock does. Granting an exception
  means deciding the clock question explicitly; silence is a reset.

The consequence worth saying out loud before anyone is surprised by it: **a P1
confirmed on day 27 costs four weeks.** That is the intended cost. If it is the
wrong cost, the window is the number to argue about, not the reset rule.

**Day 0 may not be declared until gates 0, 2, 3, 4, 6d and 8 are closed** — that
is, until the governing documents are in the tree, the two open compiler defects
are fixed with guards, the six limitation issues are dispositioned, the
`change:*` label rule runs in CI, and per-pause data exists. Declaring it earlier
buys nothing: any of those landing during the window would reset it.

---

# Part two: the eight criteria

## 0. The governing documents are in the tree

Four documents govern the release, and every criterion that cites one needs it
to exist on `main`.

| File | What it is |
|---|---|
| `RELEASE-CRITERIA-1.0.md` | This file: the bar |
| `LIMITATIONS.md` | What W# does not do, per entry |
| `COMPATIBILITY.md` | What 1.0 promises not to break |
| `docs/defects.md` | Intake, triage, verification |

**Gate.** All four files exist on `main`, and no open issue cites a path that
does not resolve. Checkable in one line:

```sh
for f in RELEASE-CRITERIA-1.0.md LIMITATIONS.md COMPATIBILITY.md docs/defects.md; do
  test -f "$f" || { echo "missing: $f"; exit 1; }
done
```

**Evidence.** The merge commit, and a comment on each open issue that cites one
of these files confirming the path it cites now resolves.

## 1. Real programs, soaked

Programs written in W#, each doing something this project actually wants done,
each exercised continuously for the freeze window. "Continuously" cannot mean the
same thing for all of them, so each gets the form of soak that fits it.

There are **three** subjects:

| Subject | What it is | Soak gate |
|---|---|---|
| Raython sample application | A long-running HTTP server | Up for the whole window with no unplanned restart, no crash, no OOM kill. Resident set at the end within 2x of the resident set at hour 24. Zero 5xx responses attributable to the runtime or the collector. |
| The `.wsharp` ecosystem check | A batch program run daily | One full run per day for every day of the window, all exiting 0, on all four release triples. |
| The compiler under its own harness | A nightly batch | `tests/harness/nightly.sh` once a day for every day of the window: parity across the three execution modes, the fuzz corpus, and the soak driver, zero failures. |

**Gate.** `scripts/soak-report.sh` prints one row per subject per day and exits
non-zero if any day is missing or any row failed. Each subject appends its own
daily TSV row; a missing row is a failure, because a soak that stopped reporting
is a soak that stopped.

**Evidence.** The soak log, and one `scripts/soak-report.sh` run showing 28
complete days for all three subjects.

**Depends on.** The Raython sample application being deployed.

## 2. A stability freeze

**Gate.** `scripts/freeze-check.sh` exits 0. It answers three questions
mechanically:

1. How many days since the recorded freeze-start commit, and is it at least 28?
2. Were any merges to `main` in that window labelled `change:breaking`?
3. Are there any open issues labelled `P1` in `sinisterMage/WSharp` or
   `sinisterMage/sharpie`?

Its output is the evidence. Every term in it is defined in Part one.

The `change:*` label rule is enforced by the `change-label` job in
`.github/workflows/release-gates.yml`. The repository owner declares day 0 and
grants any exception; see [the freeze clock](#the-freeze-clock).

**Evidence.** A `freeze-check.sh` run dated within 24 hours of the tag, exiting
0, pasted into the release checklist.

**Depends on.** The `change:*` label rule being enforced by CI from day 0, and no
P1 being open. #13, a P1 under clause 3 when this criterion was written, has
since been fixed.

## 3. Every defect gets a regression guard

Nothing is fixed without a test that would have caught it. One guard in **the
suite that owns that surface**:

| Surface | Guard lives in |
|---|---|
| Language, stdlib, the collector | `tests/cases/*.ws` |
| CLI behaviour, verbs, exit status, emitted output | `crates/wsharp-cli/tests/` |
| Runtime internals with no W#-visible surface | a unit test beside the code |
| The build and test commands themselves | a CI job |
| sharpie | `tests/*.ws` in `sinisterMage/sharpie` |
| Install scripts, artefacts, digests, the release pipeline | a CI job that fails on the unfixed version |

**Gate.** `scripts/guard-check.sh` exits 0. For every issue labelled `defect`
closed after the freeze-start commit, it asserts that the pull request which
closed it touches at least one path under that surface's guard directory. A fix
with no guard is listed by name. What the guard itself must show is
[Form A of the proof standard](#form-a--a-fix).

**Evidence.** A `guard-check.sh` run listing zero unguarded fixes.

## 4. Every remaining limitation is resolved or documented

The six issues labelled `v1.0-limitation` — #9, #10, #11, #12, #14, #15 — plus
any that join them.

**Gate.** Each has a GitHub issue that is either closed as fixed, or closed with
the label `v1.0-limitation` **and** a section in `LIMITATIONS.md` naming it,
stating what does not work, why it is not fixed for 1.0, and what would fix it.
`scripts/followups-check.sh` asserts one of those two states for each, by issue
number, and exits non-zero on one that is neither. An issue left open is a
failure; so is a closed one with no `LIMITATIONS.md` section.

What `COMPATIBILITY.md` then promises is signed off by the repository owner.

**Evidence.** `followups-check.sh` exiting 0, plus `LIMITATIONS.md` on `main`
with a section per documented limitation, each carrying
[Form C of the proof standard](#form-c--a-documented-limitation).

**Expected disposition: all six close as documented limitations.** Two are
language features whose fix is large (global storage with a startup initialiser
the collector must root; row polymorphism or an equivalent), one is an
architecture list, one needs a read-only reference the type system has no way to
express, and two are a BSD kernel behaviour and a curve whose 521 bits are not a
whole number of 32-bit limbs. That is a legitimate way to close this criterion;
it is written here so that it is a decision rather than a discovery. Any of the
six may still become a fix under Form A; none may be left undecided.

## 5. Conformance, parity and adversarial input, in CI

`cargo test --workspace` asks whether the suite passes. It does not ask whether
the three execution modes **agree with each other**, whether the front end
survives arbitrary input, or whether a long-running process creeps. Zero open
defects means those questions have not been asked, not that the answers are good.

**Gate.** `tests/harness/nightly.sh` runs in CI on a schedule and exits 0, on
`x86_64-unknown-linux-gnu` at minimum, and `tests/harness/parity.sh` runs on all
four release triples. Specifically:

1. **Parity.** For every case in `tests/cases`, `wsharp run`, `wsharp run
   --gc-stress` and `wsharp build` + execute produce the same stdout and the same
   exit status. A disagreement between two modes is a P1 under clause 1 or 3
   whichever fits, and is reported per case rather than as a total.
2. **Fuzzing.** The front end, `std/json` and `std/toml` survive the corpus with
   no abort, no hang and no crash with no W# diagnostic. A finding is a defect
   filed under `docs/defects.md`, not a line in a log.
3. **Flake rate.** `tests/harness/flake-rate.sh` reports zero non-deterministic
   cases over its repeat count. A flaky gate cannot gate.

**Evidence.** A green scheduled CI run, plus one `parity.sh` table per release
triple.

## 6. Documentation matches the implementation

"Docs match the implementation" is not measurable, so it is replaced by four
things that are.

**Gate 6a — every documented example is a real file, executed in CI.** Every
fenced W# code block in `README.md` and `docs/*.md` that is a complete program is
carried by a marker naming the file it came from:

```
<!-- from: examples/fib.ws -->
```

`scripts/check-doc-examples.sh` asserts that every such block is byte-identical
to the file it names, and that every named file is executed by the case suite or
the examples job. A block with no marker must be a fragment, and the script lists
any that is not.

**Gate 6b — there is a written compatibility statement.** `COMPATIBILITY.md`
exists on `main` and states, per surface — language syntax, inference, `std/*`,
`ingot/*`, the CLI verbs and their output, the on-disk formats, `--emit=api`, the
artefact naming scheme, the release feed — what 1.0 promises not to break, and
what is explicitly outside the promise. It cites the breaking-change definition
in Part one rather than restating it. The boundary is deliberate: **this document
defines the bar; `COMPATIBILITY.md` states the promise**, and
[Part five](#part-five-the-api-and-stability-freeze) fixes the terms it must
state.

**Gate 6c — the API surface is pinned.** The `--emit=api` golden test in
`crates/wsharp-cli/tests/api.rs` passes, and the `api::VERSION` it emits is the
version `COMPATIBILITY.md` names.

**Gate 6d — every command the repository tells a contributor to run works.** A
CI job runs each of the following and requires exit 0:

```sh
nix-shell --run "cargo build --workspace"
nix-shell --run "cargo test --workspace"
nix-shell --run "cargo test --workspace arithmetic --no-run"
```

The third is the regression guard for
[#8](https://github.com/sinisterMage/WSharp/issues/8), which is fixed: a
test-name filter used to pull `crates/wsharp-start`, which defines `main`, into a
test harness, and the link failed. A documented command that does not work is a
P2 under the scale, and the reason it gets a gate of its own rather than a line
in the limitations list is that a contributor running a filtered test reads that
failure as "my change broke the build".

**Evidence.** A `check-doc-examples.sh` run exiting 0; `COMPATIBILITY.md` on
`main` with every surface covered; a green `api.rs`; a green 6d job.

**Depends on.** The documentation at wsharp.io is outside this repository.
**Decision (2026-09-26): gate 6a covers the in-repo documentation only.**
wsharp.io is checked by whatever gates its own repository has, and the release
notes say which documentation the compatibility promise covers. Extending 6a
across repositories is post-1.0 work; pretending it is covered would be worse
than saying it is not.

## 7. Clean-machine install and the ecosystem, on four platforms

The only install that counts is one on a machine with no prior W#, no cached
toolchain, and no developer environment. Reading the release workflow is not
verification.

**Gate.** `scripts/verify-install.sh <version> <triple>` exits 0 on each of the
four release triples, run in a clean container or a fresh VM image, for both W#
and sharpie. It:

1. Downloads the published tarball and its published digest.
2. Verifies the digest **before** extracting anything, and refuses on a mismatch.
3. Extracts into the documented prefix and nowhere else.
4. Runs a hello program through `wsharp run` and again through `wsharp build`, and
   runs `ingot help`.
5. Asserts that nothing was created outside the prefix, by diffing a filesystem
   manifest taken before and after.
6. Asserts the shell profile was not edited.
7. Repeats 1–3 with a **truncated** download and with a **corrupted** one, and
   requires a refusal with a message in both cases.

And `tests/rungs.sh` in `sinisterMage/sharpie` exits 0 on each of the four
triples, one line per resolution rung: fresh install, upgrade, `+toolchain` on a
proxied command, `SHARPIE_TOOLCHAIN`, a `wsharp-toolchain.toml` found by walking
upwards, a directory override, the default, update-follows-a-channel-and-leaves-
a-pin-alone, rollback, uninstall, a rung naming a toolchain that is not installed
refusing rather than falling through, and the three fault injections (truncated
download, digest mismatch, install interrupted mid-extract) each leaving the
previous toolchain working. Each rung also asserts that `sharpie show` names the
rung that answered, since that is the first question when the answer surprises
somebody.

**Evidence.** A four-row table — one per release triple — each row carrying the
OS image, the version installed, the digest observed, the digest published, and
the exit status; plus four `rungs.sh` runs. A platform claimed by inference from
another platform's run is not a row.

**Decisions this criterion needed, both taken.**

- **Signing.** Today the release publishes a per-target `.sha256` sidecar and
  both installers check what they downloaded against it. That protects against a
  corrupted or truncated transfer and not against whoever can serve the tarball,
  because they can serve the sidecar too. **Decision (2026-09-26): 1.0
  ships with the digest sidecar and the release notes state plainly that 1.0
  promises integrity against transport corruption and not against a compromised
  distribution point.** Signing (minisign or cosign, public key in the installer,
  signature verified before the digest) is the right end state and needs a signing
  key held by the repository owner; it is scheduled for 1.1 rather than allowed to
  hold the tag. What was not acceptable was leaving it unstated.
- **Rollback.** Rollback is defined as it already works: an upgrade destroys
  nothing, so rolling back is `sharpie default <previous>`. No new verb for 1.0.

## 8. The numbers exist, and are reproducible

Three criteria above imply measurement — a resident set within 2x, a performance
regression threshold of 2x, a collector that does not stall. #18 established that
the runtime keeps a count, a total and a maximum per worker and nothing else, so
a median or a 99th percentile cannot be computed at all: the individual samples
are gone by the time anything can read them.

**Gate.** All three of:

1. **Per-pause data exists.** Either log-spaced histogram buckets in `Stats`,
   incremented in `gc::record_pause` and printed by `WSHARP_GC_STATS=1`, or an
   opt-in `WSHARP_GC_PAUSE_LOG=<path>` appending one line per pause
   (microseconds, which of the three pauses, which worker). Nothing allocates on
   the pause path either way. A unit test asserts that N recorded pauses produce N
   samples, and a case under `tests/cases` asserts the output is present and
   parseable.
2. **A published pause distribution.** p50, p90, p99 and max over at least 1,000
   pauses, produced by `tests/harness/gc-pauses.sh` on a machine whose load
   average is recorded, carrying every field
   [Form B](#form-b--a-number) requires. A single running maximum is not a
   distribution and a per-process sample is not a per-pause one.
3. **A benchmark baseline the 2x threshold can be measured against.** One
   committed baseline file per release triple, so that "a performance regression
   of more than 2x on any case in `tests/cases`" is a comparison rather than an
   impression, and a re-run command that reproduces it.

**Evidence.** The three artefacts above, each under Form B, with raw data
committed rather than summarised.

**Note on the honest caveat.** The numbers on #18 were taken on a box with a load
average of 13–16, and a pause is wall-clock time on the mutator thread, so the
tail includes descheduling. Any published number must state the load average or
be taken on a quiet machine. A number without its conditions is not evidence; see
Form B.

---

# Part three: the issue map

Revision 2 mapped every issue then open in `sinisterMage/WSharp` — nine, on
2026-09-26 — to one gate and one disposition. **All nine are now closed.**
[#13](https://github.com/sinisterMage/WSharp/issues/13), the P1 under clause 3,
was fixed by bounding the comparison into a W# diagnostic, and the bound is
documented in `LIMITATIONS.md` as "Comparing a value that reaches itself is
refused". [#8](https://github.com/sinisterMage/WSharp/issues/8) was fixed and is
guarded by gate 6d. [#18](https://github.com/sinisterMage/WSharp/issues/18) was
closed by the per-pause data criterion 8 requires.
[#9](https://github.com/sinisterMage/WSharp/issues/9),
[#10](https://github.com/sinisterMage/WSharp/issues/10),
[#11](https://github.com/sinisterMage/WSharp/issues/11),
[#12](https://github.com/sinisterMage/WSharp/issues/12),
[#14](https://github.com/sinisterMage/WSharp/issues/14) and
[#15](https://github.com/sinisterMage/WSharp/issues/15) were closed as documented
limitations, each with its entry in `LIMITATIONS.md`. The issues themselves carry
the detail.

A new issue gets a gate and a disposition before it gets work;
[docs/defects.md](docs/defects.md) is the intake.

---

# Part four: the proof standard

The rule, in one place: **every bug fix and every benchmark carries
concrete, re-runnable proof.** Three forms, one per kind of claim. A claim in any
other shape is not closed, whoever makes it and however confident they are.

## Form A — a fix

A fix is proved by a **committed test that fails before it and passes after it**.

1. The test is committed in the suite that owns the surface (criterion 3's table).
2. The pull request records, for the parent commit and for the fix commit, the
   exact command and its actual output — the failure and the pass. Both outputs,
   pasted, not described.
3. The command is one anybody can run: `nix-shell --run "cargo test --workspace"`
   or a named harness script, not a sequence reconstructed from memory.
4. A fix whose test cannot fail before it is not proved. If the defect needs
   `--gc-stress`, a specific platform or a repeat count to reproduce, the test
   carries that condition and the PR says which pass exercises it.
5. A defect found on a platform you cannot run locally is proved on that
   platform, in CI or on real hardware, and the run is linked. "It should work
   there" is not Form A.

## Form B — a number

A number is proved by **harness, hardware, raw data and a re-run command**. Every
field, every time, in a table like the one #18 already used:

| Field | Means |
|---|---|
| **Harness** | The script and its exact arguments, at a commit |
| **Compiler** | The binary and the full SHA it was built from |
| **Platform** | OS, kernel, libc, architecture, core count |
| **Load** | Load average or "quiet machine", because a pause is wall-clock time |
| **Mode** | `wsharp run`, `run --gc-stress`, or `build` + execute |
| **Samples** | How many, and of what — one per pause is not one per process |
| **Raw data** | Committed, not summarised. A percentile recomputable from the file |
| **Re-run** | One command that produces it again |

A number with a missing field is a number nobody can check, and a number nobody
can check does not go in a release note. A number whose conditions were bad — the
loaded box on #18 — may still be published **with the conditions stated**; what
may not happen is publishing it as though the conditions were good.

## Form C — a documented limitation

A limitation is proved by a **docs change plus a terminal issue state**.

1. A section in `LIMITATIONS.md` carrying all four fields that file's own format
   requires: what, why, workaround (or the word *None*), tracked.
2. The program in the entry was run against the release under discussion and the
   output shown is what it printed.
3. The issue is closed with the label `v1.0-limitation` and links the section.
4. Where a `tests/cases` entry can pin the behaviour mechanically, it exists and
   is named in the entry — so the limitation cannot quietly stop being true, or
   quietly get worse, without a test noticing.
5. A limitation whose behaviour is a crash is not documentable. It gets bounded
   into a diagnostic first (clause 3), and then the bound is documented.

---

# Part five: the API and stability freeze

What 1.0 promises, and for how long. `COMPATIBILITY.md` states this to users,
surface by surface; this section fixes the terms it must state, so that the two
cannot drift into two different promises.

## What is frozen at 1.0

Breaking change is defined in Part one. For each surface below, 1.0 promises no
breaking change for the life of the 1.x line:

| Surface | Frozen |
|---|---|
| Language syntax and semantics | Yes |
| Type inference — what is accepted, and the types inferred | Yes |
| `std/*` public names, signatures and error sets | Yes |
| `ingot/*` public names, signatures and error sets | Yes |
| `wsharp` and `ingot` verbs, flags, exit statuses, TSV columns | Yes |
| On-disk formats: lockfile, `ingot.env`, store tree hash | Yes |
| The three emitted tables, by `MAGIC`/`VERSION` | Yes |
| `--emit=api` output, at `api::VERSION` | Yes |
| Release artefact naming, the digest sidecar, the release feed | Yes |
| sharpie's verbs, rungs and resolution order | Yes |

## What is not frozen, and is said so out loud

| Surface | Why not |
|---|---|
| The runtime's C symbols | An internal boundary between the compiler and its own runtime archive. Never a public ABI. |
| Diagnostic wording and rendering | Improving a message must stay free. Nothing may parse diagnostics. |
| `--emit=ast` | A debugging aid, shared with the parser tests, free to change. `--emit=api` is the one with a promise. |
| Performance figures | The 2x threshold in P2 is a guard against a cliff, not a published figure. |
| Internal crate APIs | `wsharp-syntax`, `sema`, `codegen`, `runtime` as libraries. The compiler is the product. |
| Reproducible builds | 1.0 promises a published digest per artefact and that it matches the bytes served — not that a third party can rebuild those bytes. |
| The supported platform set | 1.0 does not add one. See the matrix below. |
| wsharp.io | Outside this repository; the release notes say which documentation the promise covers. |

## For how long

**Decision (2026-09-26).**

- **The 1.x line is source-compatible with 1.0.** A program that compiles and
  behaves correctly on 1.0 does so on every later 1.x, for every frozen surface
  above. Breaking changes wait for 2.0.
- **1.x receives P1 fixes for at least 12 months after the 1.0 tag, and until
  2.0 is tagged, whichever is later.** A P1 in the trusted surface (clause 7) or
  in the install path (clauses 5 and 6) is fixed on the 1.x line regardless of
  what is happening on `main`.
- **A 2.0 that breaks something frozen here states what, and why, in its release
  notes, against this table.** No silent renumbering.
- **Deprecation before removal.** A frozen name that is to go in 2.0 is
  deprecated in a 1.x release first, with the replacement named in the
  deprecation. Removal without a prior deprecation is not something 2.0 gets to
  do to a 1.0 program either.

This is a policy choice rather than a measurement, and it is the repository
owner's to overrule. It is written down so that it is a decision rather than an
assumption; twelve months is the number to argue about if it is wrong.

---

# The platform matrix

**Four release triples, two architectures.** Stated explicitly because
[#11](https://github.com/sinisterMage/WSharp/issues/11) is a documented
limitation by construction: `crates/wsharp-runtime/src/stackwalk.rs` carries a
`compile_error!` for any architecture that is not x86-64 or aarch64, because the
collector's root walk reads the frame pointer with inline assembly per
architecture.

| Triple | Tier | What must pass |
|---|---|---|
| `x86_64-unknown-linux-gnu` | 1 | Everything: `cargo test --workspace`, the built-case pass, the `--gc-stress` pass, `nightly.sh`, clean-machine install, the sharpie rung matrix |
| `x86_64-pc-windows-msvc` | 1 | `cargo test --workspace`, the built-case pass, the `--gc-stress` pass, `parity.sh`, clean-machine install, the rung matrix |
| `x86_64-apple-darwin` | 1 | As Windows |
| `aarch64-apple-darwin` | 1 | As Windows |

**Tier 1 means a release does not ship if that triple fails.** There is no tier 2
for 1.0: a platform that is not held to the whole bar is a platform whose users
find out on their own, and four is a small enough number to hold all of them to
it. Consequences worth stating before somebody assumes otherwise:

- **`aarch64-unknown-linux-gnu` is not a 1.0 release target.** The architecture is
  supported by the collector — it is half of what #11 allows — but no triple
  ships, because nothing in the release pipeline builds, tests or installs on
  it. Adding it post-1.0 is additive.
- **The BSDs are not release targets.** `crates/wsharp-runtime/src/sys/bsd.rs`
  exists, but no CI job builds or runs it, and
  [#14](https://github.com/sinisterMage/WSharp/issues/14) is a BSD-specific
  limitation. A BSD build may work; 1.0 promises nothing about it.
- **A platform's result is that platform's run.** Claimed-by-inference is not a
  row, in criterion 7 or anywhere else. The four `struct stat` offsets in
  `sys/bsd.rs`, which no CI job checks, are exactly why this rule exists.
- **32-bit is not supported and will not be.** A limb is 32 bits, a slot is a
  machine word, and the collector reads a frame pointer per architecture; this is
  an architecture list, not an oversight.

---

# Part six: the shared parts

## What this will cost in calendar time

The earliest possible tag is 28 days after the Raython sample application is
deployed and the harness is reporting daily, **assuming no P1 and no breaking
change in those 28 days**.

## Freeze record

| Field | Value |
|---|---|
| Day 0 commit | *not declared* |
| Day 0 date | *not declared* |
| Window | 28 days |
| Declared by | *pending — the repository owner, once gates 0, 2, 3, 4, 6d and 8 are closed* |

## Exception log

No exceptions granted. Each entry, when there is one, records: what changed, why
it could not wait, who granted it, and whether the clock reset.

## The state of the checks

One row per check a gate needs: fifteen checks for the nine criteria, because a
gate with both a mechanism and an artefact, or a script in each of two
repositories, has two things that can be separately missing. The State column
says whether each check exists; whether it *passes* is the gate's own question.

**Read against `main` at `4e9249a`, and held to the tree by
`scripts/gate-table-check.sh` on every pull request.** A row saying "written"
whose file is missing, or "not written" whose file exists, turns CI red. Two
rows name nothing this tree can check — criterion 0's inline test, and
criterion 7's `tests/rungs.sh`, which lives in another repository — and the
script reports them as unchecked on every run.

**Two spellings in the State column are load-bearing**, because the script reads
them. A verdict is the bold run `**written**` or `**not written**` at the start
of the cell, and a CI job is claimed as ``as `a`, `b` and `c` in
`.github/workflows/file.yml` ``. A cell that does not begin with a verdict fails
the check; a job named any other way is simply not checked.

| Check | Gate | State | Owed by |
|---|---|---|---|
| the four-file test in criterion 0 | 0 | **written and run** — inline above | maintainers |
| `scripts/gate-table-check.sh`, with `scripts/tests/gate-table-check.test.sh` | 0 | **written**, as `gate-table` in `.github/workflows/release-gates.yml`. It reads this table, so the table describes itself and drifting from the tree is now a red CI run | CI, on every pull request |
| `scripts/soak-report.sh`, with `scripts/soak-report-selftest.sh` | 1 | **written** (#20), and wired up as `soak-row` in `.github/workflows/nightly.yml`. What is owed is 28 days of rows, not the script | `nightly.yml`, daily |
| `scripts/ecosystem-check.sh`, with `scripts/tests/ecosystem-check.test.sh` | 1 | **written**, as `ecosystem` and `soak-row` in `.github/workflows/ecosystem.yml`. One run is sharpie built and tested (plainly, under stress, and its rungs), Foundry's own check fetching every release against its tree hash, and every published release installed, checked, built, run and tested plainly and under stress. The workflow writes one row per release triple, and `soak/subjects.tsv` names all four, so `soak-report.sh` fails a day any one of them did not report. What is owed is 28 days of rows | `ecosystem.yml`, daily |
| `scripts/freeze-check.sh` | 2 | **written**, on `main` | maintainers, within 24 hours of the tag |
| `scripts/check-change-label.sh`, with `scripts/tests/check-change-label.test.sh` | 2 | **written** (#21), as `change-label` in `.github/workflows/release-gates.yml` | CI, on every pull request |
| `scripts/guard-check.sh`, with `scripts/tests/guard-check.test.sh` | 3 | **written**, as `guards` in `.github/workflows/release-gates.yml`. Asks GitHub which defects closed after the freeze start and what closed them, so, like criterion 4's, it exits 2 when it cannot ask. The live run waits for `.github/freeze-start`; before Day 0, `--since` runs it over any window | CI |
| `scripts/followups-check.sh`, with `scripts/tests/followups-check.test.sh` | 4 | **written** (#45), as `followups` in `.github/workflows/release-gates.yml`. The only check in this table that asks GitHub rather than the tree, so it exits 2 — never 0 — when it cannot ask | CI |
| `tests/harness/nightly.sh` | 5 | **written** (#20), as `parity`, `nightly` and `soak-row` in `.github/workflows/nightly.yml`. What is owed is green runs on a schedule | `nightly.yml`, on its schedule |
| `scripts/check-doc-examples.sh`, with `scripts/tests/check-doc-examples.test.sh` | 6a | **written** (#21), as `doc-examples` and `examples` in `.github/workflows/release-gates.yml`, and **green** since #33 marked the documented programs up | CI — held |
| the 6d CI job | 6d | **written** (#21), as `contributor-commands` in `.github/workflows/release-gates.yml` | CI — held |
| `scripts/verify-install.sh` | 7 | **written** (#21), as `verify-install` in `.github/workflows/release-gates.yml`, a matrix job. What is owed is a run against a real release | maintainers, against a real release |
| `tests/rungs.sh` | 7 | **written**, in [`sinisterMage/sharpie`](https://github.com/sinisterMage/sharpie) (its PR #2, merged at `6af434d`), not in this repository — so no check here can see its state either way, and this row is one of the two the gate-table check reports as unchecked. What is owed is green runs on four triples | sharpie's CI, on four triples |
| `tests/harness/gc-pauses.sh` | 8 | **written** (#20), and called by `tests/harness/nightly.sh` | `nightly.yml` — held |
| a committed benchmark baseline per release triple | 8 | **not written** for three of the four: `benchmarks/baselines/x86_64-pc-windows-msvc.tsv`, `benchmarks/baselines/x86_64-apple-darwin.tsv` and `benchmarks/baselines/aarch64-apple-darwin.tsv` are owed. The x86-64 Linux baseline is committed beside where they go, written by the harness's bench.sh with `--record`, which keeps every sample and the Form B fields; `--compare` is the 2x check. The nightly's bench job records the other three on their runners as artifacts, to be reviewed and committed | maintainers, from the nightly bench artifacts |

The one check not yet written is gate 8's baselines for the three triples other
than x86-64 Linux.

## The tag checklist

Every line links to the evidence that closed it. An unchecked or hand-waved line
is not a tag.

These lines state the conditions a gate passes under, not whether its check has
been written — the table above is the single place that says which files exist,
so that the two cannot drift apart. A line naming a script is therefore not a
claim that the script is missing.

- [ ] 0. All four governing documents on `main`; no open issue cites a path that does not resolve; `gate-table-check.sh` exits 0, so this document's account of its own checks is the tree's.
- [ ] 1. `scripts/soak-report.sh` — 28 complete days, three subjects, zero failed rows.
- [ ] 2. `scripts/freeze-check.sh` — exits 0, run within 24 hours of the tag.
- [ ] 3. `scripts/guard-check.sh` — zero unguarded defect fixes.
- [ ] 4. `scripts/followups-check.sh` — all six resolved or documented; `LIMITATIONS.md` present.
- [ ] 5. `nightly.sh` green on a schedule; `parity.sh` on four triples; zero flaky cases.
- [ ] 6. `check-doc-examples.sh` exits 0; `COMPATIBILITY.md` present; `api.rs` green; the 6d job green.
- [ ] 7. `verify-install.sh` — four rows, four triples, digests recorded and compared; `rungs.sh` green on four triples.
- [ ] 8. A pause distribution and a benchmark baseline per triple, each under Form B.
- [ ] Every fix in the window carries Form A; every number carries Form B; every limitation carries Form C.
- [ ] The rollback path for the tag itself is written down: how to un-ship it, and who does.
- [ ] No credential appears in any workflow file, commit, or published artefact.
- [ ] The repository owner has authorised the tag in writing.

## Revision history

| Revision | Date | What changed |
|---|---|---|
| 1 | 2026-09-26 | First draft, on PR #7: seven criteria, three definitions, `freeze-check.sh`. Closed unmerged at `ce6b3ff`. |
| 2 | 2026-09-26 | Published as the release gate list. Part one preserved so existing citations still resolve — clause 3 and criterion 4 mean what #8 through #18 say they mean. Added: criterion 0 (the governing documents), criterion 8 (the numbers), gate 6d (#8), Part three (the issue map), Part four (the proof standard), Part five (the API and stability freeze), the platform matrix. Decisions recorded: clause 3 carries no exemption (#13 is a fix); 6a covers in-repo documentation only; 1.0 ships with digest sidecars and says so; rollback is `sharpie default <previous>`; criterion 1 names three soak subjects rather than four. |
| 3 | 2026-09-26 | "The state of the checks" re-read against `main` at `f2f2b40`, after #20, #21 and #23 landed behind revision 2. Criteria 7 and 8 split into one row per separately-missing check; `check-change-label.sh` added as criterion 2's second check. The table now carries the commit it was read at, and `scripts/gate-table-check.sh` holds it to the tree as the `gate-table` job. No criterion, gate or threshold changed. |
| 4 | 2026-09-29 | Rewritten for users and contributors: per-person ownership removed, Part three condensed now that all nine issues are closed, and the gate 6d note corrected now that #8 is fixed. No criterion, gate, threshold, severity definition, promise or decision changed. |
