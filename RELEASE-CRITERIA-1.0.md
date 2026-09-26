# What v1.0 means

**Status: in force for the v1.0 campaign, revision 2, 2026-09-26.** Published by
Johnny as the single release-blocking gate list. Every other v1.0 task is
measured against this file; where a task and this file disagree, this file is
either right or gets amended here, never worked around locally. Amendments are
recorded in "Revision history" at the end.

W# is at **0.2.3**. Every numbered item in [ROADMAP.md](ROADMAP.md) is marked
done, so the gap between here and a credible 1.0 is not features. It is
*sustained real use*, the defects that only sustained real use finds, and
evidence that anyone can re-run. This document is therefore a stability and
evidence bar rather than a feature checklist: it says what has to be true, how
each thing is measured, who owns it, and what artefact closes it.

A criterion that needs a judgement call is not a criterion. Each of the eight
below is written so that a script, or a person reading a script's output, can
answer yes or no without arguing. Where a criterion still contains a judgement,
that judgement is named and given an owner rather than left implicit.

## How to read a criterion

Each one carries four fields:

| Field | Means |
|---|---|
| **Gate** | The yes/no question. Where a command answers it, the command is given. |
| **Owner** | One name and one task. The person who makes the gate answer yes. |
| **Evidence** | The artefact that closes it — a run, a file, a table. Not an assertion. |
| **Depends on** | Work owned by somebody else that the gate cannot pass without. |

"Verified" in this document means a script ran and its output was recorded. A
thing checked by hand once is a thing that regresses silently, so a manual check
is not evidence; the script that encodes it is. What counts as evidence is
defined once, in [Part four: the proof standard](#part-four-the-proof-standard),
and every criterion points there.

## Where to start reading

| If you want | Read |
|---|---|
| Your own task's gate | [Part two](#part-two-the-eight-criteria), then [the issue map](#part-three-the-nine-open-issues) |
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
directly on [#13](https://github.com/sinisterMage/WSharp/issues/13) and the
answer is no, ruled by Johnny on 2026-09-26. A crash is a crash whether or not a
file says it will happen; documentation changes who is surprised, not what the
process does. A limitation may be *refused with a diagnostic* and documented —
that is a limitation. A limitation that aborts the process is a P1 and gets
bounded. The consequence for #13 is recorded in
[Part three](#part-three-the-nine-open-issues).

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

The filer proposes a severity; the triage owner for the surface confirms or
changes it within one working day:

| Surface | Triage owner |
|---|---|
| Compiler, runtime, stdlib, the collector | Mira (WLA-3, WLA-4) |
| Harness, conformance, fuzzing, differential testing | Dex (WLA-6) |
| Benchmarks, collector measurement, published numbers | Ridge (WLA-5) |
| Release pipeline, install, artefacts, digests, sharpie, `.wsharp` ecosystem | Ash (WLA-7) |
| Raython and its sample application | Wren (WLA-8) |

Disagreement about a severity goes to Johnny and is settled within one working
day. **An unconfirmed report counts at the severity the filer proposed** until it
is triaged, so "no open P1" cannot be satisfied by leaving reports untriaged.

## The freeze clock

The freeze is a **window of 28 consecutive days** ending at the tag, during which
no breaking change lands and no P1 is open.

- **Day 0** is the commit on `main` declared as the freeze start. It is recorded
  in this file, under "Freeze record", by the person who declares it (Johnny).
- The clock **resets to day 0** on either of: a merge to `main` labelled
  `change:breaking`, or the confirmation of a P1 in the compiler, runtime,
  stdlib, release pipeline, install scripts or sharpie. On a P1 the clock
  restarts when the fix **and its regression guard** are merged, not when the
  defect is diagnosed.
- The clock **does not reset** on: a P2 or P3 fix, a docs change, a test
  addition, a CI or workflow change, or a P1 confined to a driver program that is
  not itself shipped as an artefact. A driver-program P1 resets *that program's*
  soak counter under criterion 1 instead.
- Only Johnny may grant an exception, only in writing on the campaign thread, and
  every exception is recorded in the "Exception log" below with what changed, why
  it could not wait, and what the clock does. Granting an exception means
  deciding the clock question explicitly; silence is a reset.

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

The draft of this document had seven. Criterion 8 is new, because
[#18](https://github.com/sinisterMage/WSharp/issues/18) established that the
numbers three of the other criteria imply cannot currently be obtained at all,
and an unobtainable number is a gate nobody can pass. Criterion 0 is also new,
and exists because the documents this file cites were not in the tree when it was
written.

## 0. The governing documents are in the tree

Four documents govern the campaign. Until they are on `main`, every criterion
that cites one is citing a file that does not exist — which is the state this
revision was written to end.

| File | What it is | Recovered from |
|---|---|---|
| `RELEASE-CRITERIA-1.0.md` | This file: the bar | PR #7 (`ce6b3ff`), revised |
| `LIMITATIONS.md` | What W# does not do, per entry | PR #16 (`1119247`) |
| `COMPATIBILITY.md` | What 1.0 promises not to break | PR #16 (`1119247`) |
| `docs/defects.md` | Intake, triage, verification | PR #6 (`80efa89`) |

**Gate.** All four files exist on `main`, and no open issue cites a path that
does not resolve. Checkable in one line:

```sh
for f in RELEASE-CRITERIA-1.0.md LIMITATIONS.md COMPATIBILITY.md docs/defects.md; do
  test -f "$f" || { echo "missing: $f"; exit 1; }
done
```

**Owner.** Johnny.

**Evidence.** The merge commit, and a comment on each of the nine open issues
confirming the path it cites now resolves.

**Why this was needed.** The drafts were written on four branches (PRs #6, #7,
#16, #17); all four were closed unmerged on 2026-09-26 and their branches
deleted, so nine open issues cite clause numbers in files that are not on `main`.
The content survives at the commits above and is restored here rather than
rewritten, because the issues cite it by clause and a rewrite would silently
renumber what they point at. `tests/harness/` (PR #17, `39e5a76`) is restored
separately under criterion 5, because it is Dex's and Ridge's to run rather than
mine to land.

## 1. Real programs, soaked

Programs written in W#, each doing something this project actually wants done,
each exercised continuously for the freeze window. "Continuously" cannot mean the
same thing for all of them, so each gets the form of soak that fits it.

The draft named four subjects, two of which (a docs site generator, a CI
log/metrics pipeline) no task owns. An unowned soak subject is not a gate, so
this revision names **three**, each against the task that owns it:

| Subject | What it is | Soak gate | Owner |
|---|---|---|---|
| Raython sample application | A long-running HTTP server | Up for the whole window with no unplanned restart, no crash, no OOM kill. Resident set at the end within 2x of the resident set at hour 24. Zero 5xx responses attributable to the runtime or the collector. | Wren, WLA-8 |
| The `.wsharp` ecosystem check | A batch program run daily | One full run per day for every day of the window, all exiting 0, on all four release triples. | Ash, WLA-7 |
| The compiler under its own harness | A nightly batch | `tests/harness/nightly.sh` once a day for every day of the window: parity across the three execution modes, the fuzz corpus, and the soak driver, zero failures. | Dex, WLA-6 |

**Gate.** `scripts/soak-report.sh` prints one row per subject per day and exits
non-zero if any day is missing or any row failed. Each subject appends its own
daily TSV row; a missing row is a failure, because a soak that stopped reporting
is a soak that stopped.

**Owner.** Dex owns `soak-report.sh` and the collection; Wren and Ash own their
own rows.

**Evidence.** The soak log, and one `scripts/soak-report.sh` run showing 28
complete days for all three subjects.

**Depends on.** The Raython sample application does not exist yet (WLA-8). This
is the longest pole in the campaign and the criterion is honest about it rather
than discovering it in week three.

## 2. A stability freeze

**Gate.** `scripts/freeze-check.sh` exits 0. It answers three questions
mechanically:

1. How many days since the recorded freeze-start commit, and is it at least 28?
2. Were any merges to `main` in that window labelled `change:breaking`?
3. Are there any open issues labelled `P1` in `sinisterMage/WSharp` or
   `sinisterMage/sharpie`?

Its output is the evidence. Every term in it is defined in Part one.

**Owner.** Ash runs the script and owns the `change:*` label CI job; Johnny
declares day 0 and grants any exception.

**Evidence.** A `freeze-check.sh` run dated within 24 hours of the tag, exiting
0, pasted into the release checklist.

**Depends on.** The `change:*` label rule being enforced by CI from day 0, and
#13 being fixed (it is an open P1 under clause 3, so question 3 cannot pass while
it is open).

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

**Owner.** Mira for compiler, runtime and stdlib defects; Ash for release,
install and sharpie defects; Dex for anything his harness found.

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

**Owner.** Mira (WLA-4) decides fix-or-document and writes the prose; Johnny
signs off on what the promise in `COMPATIBILITY.md` then says.

**Evidence.** `followups-check.sh` exiting 0, plus `LIMITATIONS.md` on `main`
with a section per documented limitation, each carrying
[Form C of the proof standard](#form-c--a-documented-limitation).

**Expected disposition: all six close as documented limitations.** Two are
language features whose fix is large (global storage with a startup initialiser
the collector must root; row polymorphism or an equivalent), one is an
architecture list, one needs a read-only reference the type system has no way to
express, and two are a BSD kernel behaviour and a curve whose 521 bits are not a
whole number of 32-bit limbs. That is a legitimate way to close this criterion;
it is written here so that it is a decision rather than a discovery. Mira may
convert any of the six to a fix by landing one under Form A; she may not leave
one undecided.

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

**Owner.** Dex (WLA-6). `tests/harness/` was written on PR #17 and closed
unmerged at `39e5a76`; restoring it is the first step of WLA-6 and not a rewrite.

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

The third is [#8](https://github.com/sinisterMage/WSharp/issues/8) and fails
today. A documented command that does not work is a P2 under the scale, and the
reason it gets a gate of its own rather than a line in the limitations list is
that `CLAUDE.md` tells every contributor to run a filtered test and the failure
reads as "my change broke the build".

**Owner.** Johnny owns `COMPATIBILITY.md` and the prose (6b); Ash owns the CI
jobs for 6a and 6d; Mira owns the fix behind 6d and the pin at 6c.

**Evidence.** A `check-doc-examples.sh` run exiting 0; `COMPATIBILITY.md` on
`main` with every surface covered; a green `api.rs`; a green 6d job.

**Depends on.** The documentation at wsharp.io is outside this repository.
**Decision, Johnny, 2026-09-26: gate 6a covers the in-repo documentation only.**
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

**Owner.** Ash (WLA-7).

**Evidence.** A four-row table — one per release triple — each row carrying the
OS image, the version installed, the digest observed, the digest published, and
the exit status; plus four `rungs.sh` runs. A platform claimed by inference from
another platform's run is not a row.

**Decisions this criterion needed, both taken.**

- **Signing.** Today the release publishes a per-target `.sha256` sidecar and
  both installers check what they downloaded against it. That protects against a
  corrupted or truncated transfer and not against whoever can serve the tarball,
  because they can serve the sidecar too. **Decision, Johnny, 2026-09-26: 1.0
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

**Owner.** Ridge (WLA-5).

**Evidence.** The three artefacts above, each under Form B, with raw data
committed rather than summarised.

**Note on the honest caveat.** The numbers on #18 were taken on a box with a load
average of 13–16, and a pause is wall-clock time on the mutator thread, so the
tail includes descheduling. Any published number must state the load average or
be taken on a quiet machine. A number without its conditions is not evidence; see
Form B.

---

# Part three: the nine open issues

Every open issue in `sinisterMage/WSharp` as of 2026-09-26, each mapped to
exactly one gate, one disposition and one owner. An issue appears once. A tenth
issue filed tomorrow gets a row here before it gets work.

| Issue | What it is | Gate | Disposition | Owner / task |
|---|---|---|---|---|
| [#13](https://github.com/sinisterMage/WSharp/issues/13) | Comparing a cyclic struct value aborts with no W# diagnostic (P1) | 2 | **Fix** | Mira, WLA-3 |
| [#8](https://github.com/sinisterMage/WSharp/issues/8) | `cargo test --workspace <filter>` fails: two mains in `wsharp-start` (P3) | 6d | **Fix** | Mira, WLA-3 |
| [#18](https://github.com/sinisterMage/WSharp/issues/18) | No per-pause data, so a pause p99 cannot be measured | 8 | **Measurement** | Ridge, WLA-5 |
| [#9](https://github.com/sinisterMage/WSharp/issues/9) | Computed top-level `const` is rejected | 4 | **Documented limitation** | Mira, WLA-4 |
| [#10](https://github.com/sinisterMage/WSharp/issues/10) | Field access needs a known type | 4 | **Documented limitation** | Mira, WLA-4 |
| [#11](https://github.com/sinisterMage/WSharp/issues/11) | x86-64 and aarch64 only | 4 | **Documented limitation** | Mira, WLA-4 |
| [#12](https://github.com/sinisterMage/WSharp/issues/12) | A top-level `const` array can be written through an alias | 4 | **Documented limitation** | Mira, WLA-4 |
| [#14](https://github.com/sinisterMage/WSharp/issues/14) | `net.shutdown` does not stop an acceptor on the BSDs | 4 | **Documented limitation** | Mira, WLA-4 |
| [#15](https://github.com/sinisterMage/WSharp/issues/15) | `std/tls` cannot verify a chain through a P-521 key | 4 | **Documented limitation** | Mira, WLA-4 |

## #13 in detail, because it needed a ruling

#13 asked whether the severity scale should carry an exemption for a documented
crash. **It should not, and does not** — see the note under clause 3. The
disposition is therefore a fix, and it is the shape #13 itself proposed as option
1: bound the recursion in the generated exact-comparison function and panic with
a W# diagnostic naming the type, in the shape of `std/json`'s `MAX_DEPTH` and
`std/x509`'s `MAX_CHAIN` — a named bound with no knob. That converts an abort into
a reported error, which is a documentable limitation; an abort is not.

Two things for whoever picks this up. A version of this fix already exists,
written on PR #6 and closed unmerged at `80efa89`: it touches
`crates/wsharp-codegen/src/equality.rs` and `lower.rs`, adds
`tests/cases/eq_cycle_bounded.ws`, and changes `tests/cases/struct_eq.ws`. Read it
before writing a second one, and hold it to Form A anyway — recovered work is not
verified work. And `LIMITATIONS.md`'s entry "Comparing a value that reaches itself
aborts" is true of 0.2.3 and must be rewritten, not deleted, when the fix lands:
the bound is itself a limitation, and a user who meets it deserves to find it
documented.

## What the map does not contain

Nothing about closed issues #1–#7, #16 and #17. Four of those are pull requests
closed unmerged, and what happens to their content is criterion 0 (for the three
documents) and criterion 5 (for `tests/harness/`).

---

# Part four: the proof standard

The owner's constraint, in one place: **every bug fix and every benchmark carries
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
5. A defect found on a platform this machine cannot run is proved on that
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

**Decision, Johnny, 2026-09-26.**

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
  ships, because nothing in this campaign builds, tests or installs on it.
  Adding it post-1.0 is additive.
- **The BSDs are not release targets.** `crates/wsharp-runtime/src/sys/bsd.rs`
  exists and is checked but not run here, and
  [#14](https://github.com/sinisterMage/WSharp/issues/14) is a BSD-specific
  limitation. A BSD build may work; 1.0 promises nothing about it.
- **A platform's result is that platform's run.** Claimed-by-inference is not a
  row, in criterion 7 or anywhere else. The four `struct stat` offsets in
  `sys/bsd.rs` that no machine here can check are exactly why this rule exists.
- **32-bit is not supported and will not be.** A limb is 32 bits, a slot is a
  machine word, and the collector reads a frame pointer per architecture; this is
  an architecture list, not an oversight.

---

# Part six: the shared parts

## Who owns what

This document is a contract between six people, not one person's opinion.

| Owner | Task | Owes |
|---|---|---|
| Mira | WLA-3, WLA-4 | The two open compiler defects with guards (#13, #8); fix-or-document for the six limitation issues and the `LIMITATIONS.md` prose; the `--emit=api` pin (6c); triage for compiler, runtime, stdlib, the collector |
| Ridge | WLA-5 | Per-pause data and the published distribution (#18); the benchmark baselines the 2x threshold is measured against; every number under Form B |
| Dex | WLA-6 | `tests/harness/` restored and in CI; parity across the three execution modes on four triples; fuzzing; the flake rate; `soak-report.sh` and the daily collection |
| Ash | WLA-7 | The release pipeline and published digests; clean-machine install verification; sharpie and its rung matrix; the `.wsharp` ecosystem check; the `change:*` label CI job; the 6a and 6d CI jobs |
| Wren | WLA-8 | Raython and the deployed sample application, and its soak row for the whole window |
| Johnny | WLA-2 | This document and `COMPATIBILITY.md`; declares day 0; grants or refuses exceptions; settles severity disagreements; **decides the release date** |

A criterion whose owner is not on this list is a criterion nobody owns, which is
the state criterion 0 was written to end.

## What this will cost in calendar time

Worth saying once, plainly, because it is the number a release date is built
from. Criterion 1 requires three subjects soaking for 28 days, and one of them —
the Raython sample application — is not written. So the earliest possible tag is:

> (time for Wren to build and deploy Raython's sample app) + (time for Dex to
> stand up the harness and the daily report) + 28 days, **assuming no P1 and no
> breaking change in those 28 days**.

Every confirmed P1 in the window adds up to 28 more days. The freeze window is
the one number in this document that is a policy choice rather than a
measurement, and it is the one to argue about if the date comes out wrong.

## Freeze record

| Field | Value |
|---|---|
| Day 0 commit | *not declared* |
| Day 0 date | *not declared* |
| Window | 28 days |
| Declared by | *pending — Johnny, once gates 0, 2, 3, 4, 6d and 8 are closed* |

## Exception log

No exceptions granted. Each entry, when there is one, records: what changed, why
it could not wait, who granted it, and whether the clock reset.

## The state of the checks

Nine gates, nine checks. Two exist; the rest are named here so that they are owed
rather than assumed.

| Check | Gate | State | Owed by |
|---|---|---|---|
| the four-file test in criterion 0 | 0 | **written and run** — inline above | Johnny |
| `scripts/soak-report.sh` | 1 | not written | Dex |
| `scripts/freeze-check.sh` | 2 | **written**, restored on this branch | Ash to run |
| `scripts/guard-check.sh` | 3 | not written | Ash |
| `scripts/followups-check.sh` | 4 | not written | Ash, against Mira's decisions |
| `tests/harness/nightly.sh` | 5 | written on PR #17, closed unmerged at `39e5a76` | Dex to restore |
| `scripts/check-doc-examples.sh` | 6a | not written | Ash, against Johnny's prose |
| the 6d CI job | 6d | not written | Ash, against Mira's fix |
| `scripts/verify-install.sh`, `tests/rungs.sh` | 7 | not written | Ash |
| `tests/harness/gc-pauses.sh` + baselines | 8 | harness written on PR #17; baselines not | Ridge |

A criterion whose check is not written is a criterion nobody can fail, which is
why this table is here rather than in somebody's head.

## The tag checklist

Every line links to the evidence that closed it. An unchecked or hand-waved line
is not a tag.

- [ ] 0. All four governing documents on `main`; no open issue cites a path that does not resolve.
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
- [ ] Johnny has authorised the tag in writing.

## Revision history

| Revision | Date | What changed |
|---|---|---|
| 1 | 2026-09-26 | Written by Milo on PR #7. Seven criteria, three definitions, `freeze-check.sh`. Closed unmerged at `ce6b3ff`. |
| 2 | 2026-09-26 | Published by Johnny as the campaign's gate list. Part one preserved so existing citations still resolve — clause 3 and criterion 4 mean what #8 through #18 say they mean. Added: criterion 0 (the governing documents), criterion 8 (the numbers), gate 6d (#8), Part three (the nine open issues), Part four (the proof standard), Part five (the API and stability freeze), the platform matrix. Owners remapped from the campaign's earlier roster to its current one. Rulings recorded: clause 3 carries no exemption (#13 is a fix); 6a covers in-repo documentation only; 1.0 ships with digest sidecars and says so; rollback is `sharpie default <previous>`; criterion 1 drops two unowned soak subjects. |
