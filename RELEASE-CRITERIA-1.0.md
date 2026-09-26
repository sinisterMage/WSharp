# What v1.0 means

**Status: draft, awaiting ratification.** Nothing here is in force until it is
ratified on the v1.0 campaign thread. Until then it is a proposal to argue with,
which is the point of writing it down.

Every numbered item in [ROADMAP.md](ROADMAP.md) is marked done and this
repository has no open issues. So the gap between here and a credible 1.0 is not
features. It is *sustained real use*, and the defects that only sustained real
use finds. This document is therefore a stability and evidence bar rather than a
feature checklist: it says what has to be true, how each thing is measured, who
owns it, and what artefact closes it.

A criterion that needs a judgement call is not a criterion. Each of the seven
below is written so that a script, or a person reading a script's output, can
answer yes or no without arguing. Where a criterion still contains a judgement
-- and two of them do -- that judgement is named and given an owner rather than
left implicit.

## How to read a criterion

Each one carries four fields:

| Field | Means |
|---|---|
| **Gate** | The yes/no question. Where a command answers it, the command is given. |
| **Owner** | One name. The person who makes the gate answer yes. |
| **Evidence** | The artefact that closes it -- a run, a file, a table. Not an assertion. |
| **Depends on** | Work owned by somebody else that the gate cannot pass without. |

"Verified" in this document means a script ran and its output was recorded.
A thing checked by hand once is a thing that regresses silently, so a manual
check is not evidence; the script that encodes it is.

---

# Part one: definitions

These three definitions are load-bearing. Criterion 2 is a gate on both of the
first two, and the defect intake process cites the severity scale rather than
restating it -- there is one definition, in this file, and everything else
points here. If the intake and this document ever disagree, this document is
wrong and gets fixed; two definitions is the failure mode being avoided.

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
  unambiguously now ambiguous -- or, worse, resolve to a different member.
  Adding an overload is not automatically additive in a language with multiple
  dispatch.
- Changing a type's place in the dispatch lattice, or a struct's field order.
- Changing the layout or content of any format something else reads: the three
  emitted tables (`MAGIC`/`VERSION` in `wsharp-runtime/src/aot.rs`), `ingot.env`,
  the lockfile, a store tree hash, `--emit=api` output, the release archive's
  internal layout, the artefact naming scheme, or the shape of the release feed
  sharpie reads.
- Changing a CLI verb's name, a flag's meaning, an exit status, or the columns
  of a TSV line, for `wsharp`, `ingot` or `sharpie`.

And each of these is **not** breaking:

- Adding a name, a module, a flag, or an overload that leaves every existing
  call site resolving exactly as it did.
- Fixing a P1. A program that depended on a miscompilation was never correct,
  and saying otherwise would make the severest class of defect unfixable.
- Diagnostic wording, performance, internal refactoring, tests, CI.

**Mechanism.** Every pull request carries exactly one of the labels
`change:breaking`, `change:additive`, `change:fix`, `change:none`. CI fails a
pull request carrying none or more than one. This is what makes "was there a
breaking change in the window" a query rather than a memory.

## The severity scale

Severity is decided by **what the defect does**, not by how annoying it is or
how hard it is to hit. Rarity is not a mitigation; a silently wrong answer that
happens once a month is still a silently wrong answer.

### P1 -- blocks the tag, and resets the freeze clock

Any one of the following:

1. **A silently wrong answer.** A program the compiler accepts produces an
   incorrect result with no diagnostic: the wrong overload selected, wrong
   arithmetic, a field read from the wrong offset, a wrong comparison. The
   dispatched-call defect written up in item 13 of ROADMAP.md is the canonical
   instance.
2. **Memory unsafety.** The collector frees, moves or fails to trace a reachable
   object; generated code reads through a stale or unbarriered reference; a
   stack map describes a slot wrongly. **Any abort under `--gc-stress` is P1 by
   definition**, with no further argument about whether it could happen without
   stress.
3. **A crash with no W# diagnostic** -- SIGSEGV, SIGILL, `0xC0000005`, an abort,
   or a hang with no progress -- from a program that uses no FFI.
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
   download path: accepting a certificate chain that must be refused, accepting
   a forged signature or AEAD tag, or transmitting key material in clear.
8. **A credential in a workflow file, a commit, or a published artefact.**

### P2 -- does not block the tag on its own; must be fixed or documented

- The compiler rejects a program it should accept, or reports the wrong cause.
- A stdlib function returns a wrong answer for inputs inside its documented
  domain but the wrongness is *reported* rather than silent (an error, a panic
  with a message).
- A platform arm fails where another passes, for a non-P1 reason.
- A performance regression of more than 2x on any case in `tests/cases`.
- A documented example that does not run.

An open P2 at the tag must appear in the release notes' limitations list. An
open P2 that is neither fixed nor listed is a P1 against criterion 6, because it
makes the documentation wrong.

### P3 -- cosmetic

Wording, formatting, ergonomics, a diagnostic that is correct but could be
kinder. Does not gate anything.

### Who decides

The filer proposes a severity; the triage owner for the surface confirms or
changes it within one working day:

| Surface | Triage owner |
|---|---|
| Compiler, runtime, stdlib, the collector | Ada |
| Release pipeline, install scripts, artefacts, digests, sharpie | Milo |
| A driver program's own defect | Rex |
| Harness, fuzzing, soak infrastructure | Vera |

Disagreement about a severity goes to Johnny and is settled within one working
day. **An unconfirmed report counts at the severity the filer proposed** until
it is triaged, so "no open P1" cannot be satisfied by leaving reports untriaged.

## The freeze clock

The freeze is a **window of 28 consecutive days** ending at the tag, during
which no breaking change lands and no P1 is open.

- **Day 0** is the commit on `main` declared as the freeze start. It is recorded
  in this file, under "Freeze record", by the person who declares it.
- The clock **resets to day 0** on either of: a merge to `main` labelled
  `change:breaking`, or the confirmation of a P1 in the compiler, runtime,
  stdlib, release pipeline, install scripts or sharpie. On a P1 the clock
  restarts when the fix **and its regression guard** are merged, not when the
  defect is diagnosed.
- The clock **does not reset** on: a P2 or P3 fix, a docs change, a test
  addition, a CI or workflow change, or a P1 confined to a driver program that
  is not itself shipped as an artefact. A driver-program P1 resets *that
  program's* soak counter under criterion 1 instead.
- Only Johnny may grant an exception, only in writing on the campaign thread,
  and every exception is recorded in the "Exception log" below with what
  changed, why it could not wait, and what the clock does. Granting an exception
  means deciding the clock question explicitly; silence is a reset.

The consequence worth saying out loud before anyone is surprised by it: **a P1
confirmed on day 27 costs four weeks.** That is the intended cost. If it is the
wrong cost, the window is the number to argue about, not the reset rule.

---

# Part two: the seven criteria

## 1. Four real programs, soaked

Four programs written in W#, each doing something this project actually wants
done, each exercised continuously for the freeze window.

"Continuously" cannot mean the same thing for all four, and pretending it does
is how a criterion stops being measurable. Two of these are long-running
processes and two are not; each gets the form of soak that fits it.

| Program | What it is | Soak gate |
|---|---|---|
| `ingot` registry/index server | A long-running process | Up for the whole window with no unplanned restart, no crash, no OOM kill. Resident set at the end within 2x of the resident set at hour 24. Zero 5xx responses attributable to the runtime or the collector. |
| W# docs static site generator | A batch program | At least one build per day for every day of the window, all exiting 0, and the rendered output byte-identical across two runs over unchanged input. |
| CI log/metrics pipeline | A streaming process | Up for the whole window; every day, the count of records it emits equals the count of records independently fed to it. A dropped record is a failure, not a rounding error. |
| sharpie | A CLI | The criterion 7 rung matrix run once a day on all four platforms for every day of the window, with zero failures. |

**Gate.** `scripts/soak-report.sh` prints one row per program per day and exits
non-zero if any day is missing or any row failed. Each program appends its own
daily TSV row; a missing row is a failure, because a soak that stopped reporting
is a soak that stopped.

**Owner.** Rex owns the three driver programs; Milo owns sharpie's rows; Vera
owns the harness that collects and reports.

**Evidence.** The soak log, and one `scripts/soak-report.sh` run showing 28
complete days for all four.

**Depends on.** Three of these four programs do not exist yet. See "What this
will cost in calendar time" below -- this is the longest pole in the campaign
and the criterion is honest about it rather than discovering it in week three.

## 2. A stability freeze

**Gate.** `scripts/freeze-check.sh` exits 0. It answers three questions
mechanically:

1. How many days since the recorded freeze-start commit, and is it at least 28?
2. Were any merges to `main` in that window labelled `change:breaking`?
3. Are there any open issues labelled `P1` in `sinisterMage/WSharp` or
   `sinisterMage/sharpie`?

Its output is the evidence. Every term in it is defined in Part one.

**Owner.** Milo writes and runs the script; Johnny declares day 0 and grants any
exception.

**Evidence.** A `freeze-check.sh` run dated within 24 hours of the tag,
exiting 0, pasted into the release checklist.

**Depends on.** The `change:*` label rule being enforced by CI from day 0. A
window whose merges were not labelled cannot be checked after the fact, so this
mechanism has to be in place *before* the freeze starts, not at the end of it.

## 3. Every defect gets a regression guard

Nothing is fixed without a test that would have caught it.

The seven-point bar says "a minimal `tests/cases` entry", and for a language or
stdlib defect that is exactly right. But `tests/cases` is the compiler's suite
and there are defects with no home in it -- an install-script defect, a release
workflow defect, a sharpie defect. So the rule is one guard in **the suite that
owns that surface**:

| Surface | Guard lives in |
|---|---|
| Language, stdlib, the collector | `tests/cases/*.ws` |
| CLI behaviour, verbs, exit status, emitted output | `crates/wsharp-cli/tests/` |
| Runtime internals with no W#-visible surface | a unit test beside the code |
| sharpie | `tests/*.ws` in `sinisterMage/sharpie` |
| Install scripts, artefacts, digests, the release pipeline | a CI job that fails on the unfixed version |

**Gate.** `scripts/guard-check.sh` exits 0. For every issue labelled `defect`
closed after the freeze-start commit, it asserts that the pull request which
closed it touches at least one path under that surface's guard directory. A fix
with no guard is listed by name.

**Owner.** Ada for compiler, runtime and stdlib defects; Milo for release,
install and sharpie defects; Vera for anything her harness found.

**Evidence.** A `guard-check.sh` run listing zero unguarded fixes.

## 4. The four smaller follow-ups are resolved or documented

The four open items under "Smaller follow-ups" in ROADMAP.md:

1. Computed top-level `const`.
2. Field access needs a known type.
3. x86-64 and aarch64 only.
4. A top-level `const` array can be written through an alias.

**Gate.** Each has a GitHub issue that is either closed as fixed, or closed with
the label `v1.0-limitation` **and** a section in `LIMITATIONS.md` naming it,
stating why it is not fixed for 1.0, and saying what would fix it.
`scripts/followups-check.sh` asserts one of those two states for each of the
four, by issue number, and exits non-zero on a follow-up that is neither.

**Owner.** Ada decides fix-or-document for 1, 2 and 4; 3 is architecture support
and is a documented limitation by construction. Iris writes the `LIMITATIONS.md`
prose for whichever are documented.

**Evidence.** `followups-check.sh` exiting 0, plus `LIMITATIONS.md` in the tree.

**Expect at least three of the four to close as documented limitations.** Two
are language features whose fix is large (global storage with a startup
initialiser that the collector must root; row polymorphism or an equivalent),
and one is an architecture list. That is a legitimate way to close this
criterion; it is written here so that it is a decision rather than a discovery.

## 5. Clean-machine install, on four platforms, verified by script

The only install that counts is one on a machine with no prior W#, no cached
toolchain, and no developer environment. Reading the release workflow is not
verification.

**Gate.** `scripts/verify-install.sh <version> <triple>` exits 0 on each of the
four release triples, run in a clean container or a fresh VM image, for both W#
and sharpie. It:

1. Downloads the published tarball and its published digest.
2. Verifies the digest **before** extracting anything, and refuses on a
   mismatch.
3. Extracts into the documented prefix and nowhere else.
4. Runs a hello program through `wsharp run` and again through `wsharp build`,
   and runs `ingot help`.
5. Asserts that nothing was created outside the prefix, by diffing a filesystem
   manifest taken before and after.
6. Asserts the shell profile was not edited.
7. Repeats 1-3 with a **truncated** download and with a **corrupted** one, and
   requires a refusal with a message in both cases.

**Owner.** Milo.

**Evidence.** A four-row table -- `x86_64-unknown-linux-gnu`,
`x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, `aarch64-apple-darwin` -- each
row carrying the OS image, the version installed, the digest observed, the
digest published, and the exit status. A platform claimed by inference from
another platform's run is not a row.

**Depends on.** Vera for the cross-platform matrix wiring; a decision from
Johnny on the signing question below.

**The integrity promise needs a decision before the tag.** Today the release
publishes a per-target `.sha256` sidecar beside each tarball, and both installers
check what they downloaded against it. That protects against a corrupted or
truncated transfer, which is worth having and is most of what goes wrong. It
does **not** protect against whoever can serve the tarball, because they can
serve the sidecar too. For 1.0 there are two honest options and no third:

- Sign the digests -- minisign or cosign -- ship the public key in the
  installer, and verify the signature before the digest. This needs a signing
  key as a repository secret, which is Johnny's to create.
- Or state plainly in the release notes that 1.0 promises integrity against
  transport corruption and not against a compromised distribution point.

What is not acceptable is leaving it unstated. **Milo recommends the first.**

## 6. Documentation matches the implementation

"Docs match the implementation" is not measurable, so it is replaced by three
things that are.

**Gate 6a -- every documented example is a real file, executed in CI.** Every
fenced W# code block in `README.md` and `docs/*.md` that is a complete program
is carried by a marker naming the file it came from:

```
<!-- from: examples/fib.ws -->
```

`scripts/check-doc-examples.sh` asserts that every such block is byte-identical
to the file it names, and that every named file is executed by the case suite or
the examples job. A block with no marker must be a fragment, and the script
lists any that is not. The documentation cannot then drift from the
implementation, because CI diffs it against the thing that runs.

**Gate 6b -- there is a written compatibility statement.** `COMPATIBILITY.md`
exists and states, per surface -- language syntax, inference, `std/*`,
`ingot/*`, the CLI verbs and their output, the on-disk formats, `--emit=api`,
the artefact naming scheme, the release feed -- what 1.0 promises not to break,
and what is explicitly outside the promise. It cites the breaking-change
definition in Part one rather than restating it. The boundary with this document
is deliberate: **this document defines the bar; `COMPATIBILITY.md` states the
promise.**

**Gate 6c -- the API surface is pinned.** The `--emit=api` golden test in
`crates/wsharp-cli/tests/api.rs` passes, and the `api::VERSION` it emits is the
version `COMPATIBILITY.md` names.

**Owner.** Iris owns `COMPATIBILITY.md` and the documentation prose; Milo owns
the CI job for 6a; Ada owns 6c.

**Evidence.** A `check-doc-examples.sh` run exiting 0; `COMPATIBILITY.md` in the
tree with every surface covered; a green `api.rs`.

**Depends on.** The documentation at wsharp.io is outside this repository. If
its source is not somewhere CI can gate on, 6a covers the in-repo documentation
only and wsharp.io is checked by whatever gates that site's own repository has.
**This needs an answer from Johnny or Iris before the criterion can be called
complete**, and the honest default is to say in the release notes which
documentation the promise covers.

## 7. sharpie 1.0: every resolution rung verified

A version manager that can move forward but not back is a trap. Every rung is
exercised, not just the happy one, and against the real release feed rather than
a fixture.

**Gate.** `tests/rungs.sh` in `sinisterMage/sharpie` exits 0 on each of the four
triples. One line per rung, each with the exact command:

| # | Rung | Command |
|---|---|---|
| 1 | Fresh install onto a clean machine | `sharpie install stable` |
| 2 | Upgrade over an existing install | `sharpie install <newer>` with an older present |
| 3 | `+toolchain` on a proxied command | `wsharp +<version> build x.ws` |
| 4 | Environment variable | `SHARPIE_TOOLCHAIN=<version> wsharp build x.ws` |
| 5 | Directory file | a `wsharp-toolchain.toml` found by walking upwards |
| 6 | Directory override | `sharpie override set <version>` |
| 7 | Default | `sharpie default <version>` |
| 8 | Update follows a channel, leaves a pin alone | `sharpie update` with one of each installed |
| 9 | Rollback | see below |
| 10 | Uninstall | `sharpie uninstall <version>` |
| 11 | A rung naming a toolchain that is not installed refuses rather than falling through | each of rungs 3-7 |
| 12 | A truncated download refuses and leaves the previous toolchain working | fault injection |
| 13 | A digest mismatch refuses and leaves the previous toolchain working | fault injection |
| 14 | An install interrupted mid-extract leaves the previous toolchain working | kill during `install` |

Each rung also asserts that `sharpie show` names the rung that answered, since
that is the first question when the answer surprises somebody.

**Owner.** Milo.

**Evidence.** Four `rungs.sh` runs, one per triple, each printing 14 pass lines.

**Rung 9 does not exist yet and this needs a decision.** sharpie's verbs today
are `show`, `default`, `toolchain`, `init`, `install`, `uninstall`, `update`,
`override`. There is no `rollback`. There are two ways to make the criterion
true:

- **Define rollback as it already works**: the previous toolchain is still
  installed after an upgrade, so rolling back is `sharpie default <previous>`.
  That is testable today and is arguably the honest design -- nothing is
  destroyed by an upgrade, so nothing needs undoing.
- **Add a `sharpie rollback` verb** that moves the default to the
  previously-defaulted toolchain, which requires recording what that was.

This document assumes the first, and rung 9 is written against it. If Johnny or
Rex wants the verb, it is new work on the sharpie side and belongs on WLA-7.

---

# Part three: the shared parts

## Who owns what

This document is a contract between five people, not one person's opinion. Where
a criterion depends on somebody else's work, it says so above; collected here:

| Owner | Owes |
|---|---|
| Rex | The three driver programs of criterion 1 -- the registry server, the docs site generator, the CI log/metrics pipeline -- built and deployed before the freeze window opens. |
| Vera | The soak harness and its daily report (criterion 1); the cross-platform matrix the install verification runs on (criterion 5); adversarial input to the download path. |
| Ada | Fix-or-document for the smaller follow-ups (criterion 4); the fixes and regression guards for every compiler, runtime and stdlib P1 (criteria 2 and 3); the `--emit=api` pin (6c). Cites the severity scale in Part one rather than defining a second one. |
| Iris | `COMPATIBILITY.md` (6b); `LIMITATIONS.md` prose (criterion 4); the release notes' limitations list. |
| Milo | Freeze mechanics and `freeze-check.sh`; `guard-check.sh`; clean-machine install verification (criterion 5); sharpie and its rung matrix (criterion 7); the release pipeline and the published digests. |
| Johnny | Declares day 0; grants or refuses exceptions; decides the signing question in criterion 5; decides the wsharp.io documentation scope in criterion 6; **decides the release date**. |

## What this document does not promise

Stating these here is cheaper than being asked at the tag.

- **Reproducible builds are not a 1.0 criterion.** A Rust release build is not
  bit-identical across machines without work this campaign has not scoped. What
  1.0 promises is a *published digest per artefact* and that the digest matches
  the bytes served -- not that a third party can rebuild those bytes. If that is
  wanted, it is its own piece of work and it should be said now, not at the tag.
- **1.0 does not promise performance numbers.** The 2x regression threshold in
  the P2 definition is a guard against a cliff, not a published figure.
- **1.0 does not promise ABI stability for the runtime's C symbols.** They are
  an internal boundary between the compiler and its own runtime archive.
- **1.0 does not add a supported platform.** The four release triples are the
  four that exist; x86-64 and aarch64 remain the only architectures.

## What this will cost in calendar time

Worth saying once, plainly, because it is the number a release date is built
from. Criterion 1 requires four programs soaking for 28 days, and three of those
programs are not written. So the earliest possible tag is:

> (time for Rex to build and deploy three programs) + (time for Vera to stand up
> the soak harness) + 28 days, **assuming no P1 and no breaking change in those
> 28 days**.

Every confirmed P1 in the window adds up to 28 more days. The freeze window is
the one number in this document that is a policy choice rather than a
measurement, and it is the one to argue about if the date comes out wrong.

## Freeze record

| Field | Value |
|---|---|
| Day 0 commit | *not declared* |
| Day 0 date | *not declared* |
| Window | 28 days |
| Declared by | *pending* |

## Exception log

No exceptions granted. Each entry, when there is one, records: what changed, why
it could not wait, who granted it, and whether the clock reset.

## The state of the seven scripts

Seven criteria, seven checks. One exists; the other six are named here so that
they are owed rather than assumed, and each is listed against the person who
owes it.

| Check | State | Owed by |
|---|---|---|
| `scripts/freeze-check.sh` | **written and run** -- see the run output on the ratification thread | Milo |
| `scripts/guard-check.sh` | not written | Milo |
| `scripts/verify-install.sh` | not written | Milo |
| `scripts/followups-check.sh` | not written | Milo, against Ada's decisions |
| `scripts/check-doc-examples.sh` | not written | Milo, against Iris's documentation |
| `scripts/soak-report.sh` | not written | Vera |
| `tests/rungs.sh` (sharpie) | not written | Milo |

A criterion whose check is not written is a criterion nobody can fail, which is
why this table is here rather than in somebody's head.

## The tag checklist

Every line links to the evidence that closed it. An unchecked or hand-waved line
is not a tag.

- [ ] 1. `scripts/soak-report.sh` -- 28 complete days, four programs, zero failed rows.
- [ ] 2. `scripts/freeze-check.sh` -- exits 0, run within 24 hours of the tag.
- [ ] 3. `scripts/guard-check.sh` -- zero unguarded defect fixes.
- [ ] 4. `scripts/followups-check.sh` -- all four resolved or documented; `LIMITATIONS.md` present.
- [ ] 5. `scripts/verify-install.sh` -- four rows, four triples, digests recorded and compared.
- [ ] 6. `scripts/check-doc-examples.sh` exits 0; `COMPATIBILITY.md` present; `api.rs` green.
- [ ] 7. `tests/rungs.sh` -- four triples, fourteen rungs each, all pass.
- [ ] The rollback path for the tag itself is written down: how to un-ship it, and who does.
- [ ] No credential appears in any workflow file, commit, or published artefact.
- [ ] Johnny has authorised the tag in writing.
