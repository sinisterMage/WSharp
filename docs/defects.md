# Defects: intake, severity, and triage

How a defect gets from "this program did the wrong thing" to a merged fix with a
test that would have caught it.

This document is the definition of **severity** for this project. The release
criteria cite it rather than restating it: two copies of a P1 definition is a gap
between a freeze gate and a release gate, and the gap is discovered at the worst
possible moment.

## Where a defect lands

**GitHub issues on [`sinisterMage/WSharp`](https://github.com/sinisterMage/WSharp/issues)
are the record.** Every defect gets one, whoever found it — a user, a driver
program, the soak harness, or release verification.

The repository is the record rather than any internal tracker for one reason: a
defect history that lives somewhere private is invisible to the people deciding
whether to depend on this language, and it evaporates when whatever produced it
moves on. An issue filed today is still readable in five years by someone asking
"was this ever broken, and when was it fixed". That is most of what a defect
record is *for*.

Internal task trackers may mirror an issue where somebody needs to own the fix on
a schedule. When they do, the task links the issue and the issue is still the
place the reproduction, the cause and the fix are written down. The mirror
carries scheduling; it does not carry facts.

## What an issue must carry

The [defect report form](../.github/ISSUE_TEMPLATE/defect.yml) asks for these,
and they are the fields because each one has cost a real round trip:

| Field | Why |
|---|---|
| **Minimal reproduction** | The smallest `.ws` program that still fails. This is the one that matters: the reduction usually *is* the regression case, so a reported defect that arrives reduced arrives half fixed. |
| **Commands and actual output** | Pasted, not paraphrased. A paraphrased error message is a different error message. |
| **Expected output** | What should have happened. Sometimes writing this down is what reveals the behaviour is correct and the expectation was wrong, which is a cheaper way to find that out than a day of debugging. |
| **Commit SHA** | The full 40 characters. "Latest main" names a different program each day. |
| **Execution mode** | `run` (JIT), `run --gc-stress`, or a `build` executable (AOT). Untried is not the same as passing, so the form asks you to tick only what you ran. |
| **Platform** | OS, architecture, and the libc version on Linux. Issue #4 was a glibc 2.41 reproduction against green CI; without the libc version it would have read as "works for me". |
| **Severity** | Proposed by the reporter, confirmed at triage. |

Issues [#2](https://github.com/sinisterMage/WSharp/issues/2),
[#3](https://github.com/sinisterMage/WSharp/issues/3) and
[#4](https://github.com/sinisterMage/WSharp/issues/4) are what a good report
looks like. #2 is the best of them: a nine-line reproduction, all three execution
modes, and the two source ranges that disagree.

## Severity

Severity answers one question — *how bad is it if this ships* — and nothing else.
It is not urgency, not effort, and not how annoyed the reporter is. A one-line fix
can be P1 and a month of work can be P3.

### P1 — must not ship

**A defect is P1 if it meets any one of these:**

1. **Wrong answer with no diagnostic.** A program the compiler accepted produces
   an incorrect observable result and says nothing. This includes choosing the
   wrong overload, reading or writing the wrong memory location, and arithmetic
   that disagrees with the language's stated semantics.
2. **Memory unsafety or collector unsoundness.** A reachable object is freed,
   moved without its references being fixed, or missed by a root walk; the heap
   is corrupted; generated code dereferences a stale or wild pointer. A root walk
   that finds *no* roots counts, because it makes every root check pass
   vacuously.
3. **Crash on valid input.** The compiler or runtime aborts, segfaults, or panics
   on a program it should accept. A deliberate W# panic carrying a message — an
   index out of bounds, a division by zero — is the language working, and is not
   this. A Rust panic escaping the compiler always is.
4. **Data loss.** A program or tool destroys or corrupts data it was meant to
   preserve: an `ingot` store or install, a file a `std` call wrote, a lockfile.
5. **JIT/AOT divergence.** The same program produces different observable results
   under `wsharp run` and a `wsharp build` executable. Always a defect, even when
   both outputs look plausible — `codegen::build` is the only place either
   backend's code comes from, so a divergence means something below it asked
   which backend it was in.
6. **A documented guarantee is contradicted.** Behaviour disagrees with a promise
   stated in the README, the language reference, `docs/`, or the compatibility
   statement. The promise being wrong is a fix to the document, but it is this
   severity until somebody decides which side was wrong.

**No open P1 is a release gate and a freeze gate.** That is the whole reason this
list is written as six testable clauses rather than as a sentence about
seriousness: a criterion that needs a judgement call is not a criterion.

Ambiguity is resolved *upward*. A defect that might be a miscompile is P1 until
someone demonstrates it is not.

### P2 — major

- A valid program is rejected: a type error on a program that should compile, an
  inference failure, a spurious ambiguity between overloads.
- A documented feature does not work at all.
- A supported platform fails where the others pass, and the cause is not one of
  the P1 clauses.
- A hang, an unbounded pause, or memory growth without bound on a workload that
  should be flat.

Not P1 because the program does not *run wrongly* — it does not run, and the
failure is in front of you rather than behind you. That distinction is the line
between the two levels.

### P3 — minor

- A diagnostic that is correct but unreadable, unlocated, or misleading. A
  compiler's diagnostics are its user interface, so these are real defects; they
  are P3 because the rejection itself was right.
- A performance regression that is measurable and does not break a stated bound.
- Cosmetic output problems.

### P4 — trivial

Typos, dead code, cleanups, and things that would be nice.

## Triage

**Weekly, and nothing sits untriaged for a week.** Every open issue leaves triage
with three things:

- a **severity** label (`P1`–`P4`), replacing `needs-triage`,
- an **owner**, as the GitHub assignee,
- a **next action**, written as a comment — not "investigate", but the actual next
  step: reduce it further, ask the reporter for the libc version, write the
  failing case, bisect between two named commits.

A defect nobody can reproduce is a triage task, not a fix task. Say so on the
issue and ask for the exact input, platform and mode. It stays open and keeps its
proposed severity; "cannot reproduce" is not a resolution while the reporter can
still reproduce it.

Labels beyond severity: `area: syntax`, `area: sema`, `area: codegen`,
`area: runtime`, `area: cli`, `area: start`, `area: stdlib` for the crate or
library the cause sits in, applied once the cause is known rather than guessed.

## Every fix lands with a guard

**No fix merges without a regression case that fails before it and passes after.**
This is not a preference. A fix without a guard is a fix that comes back, and the
second occurrence is always more expensive than the first because everyone has
forgotten the reasoning.

Usually the guard is a `.ws` file in `tests/cases/` — the reduction from the
issue, with its expectations in a header comment. `tests/cases` is the right home
for anything observable from a W# program, because the harness runs each case
three ways: JIT, JIT under `--gc-stress`, and compiled with `wsharp build`. One
file therefore guards all three execution modes, which is exactly the coverage a
miscompile or a collector defect needs.

A crate test is right instead when the defect is not observable from a W# program
— a serialisation round trip, a platform arm, a parser dump.

Name it for the defect rather than for the feature: the next person to read it
needs to know what it is protecting.

**Write the guard before the fix, and watch it fail.** A guard that has never
failed has not been shown to guard anything, and one written afterwards
frequently does not — it is easy to write a case that passes for a reason
unrelated to the bug.

The fix itself names, in the issue, which crate was wrong and which invariant it
violated. `CLAUDE.md` is the list of invariants this project actually runs on; if
the one that broke is not in it, add it there in the same change.

## Verification

Run the checks that prove *this* fix, not the whole suite by reflex:

```sh
# The guard, and the crate the fix is in.
cargo build --workspace            # before any test run — see below
cargo test -p wsharp-<crate>
cargo test --workspace case_name

# Touched the runtime or the collector? Also:
cargo test --workspace -- --ignored gc
./target/debug/wsharp run --gc-stress tests/cases/<your case>.ws
```

`cargo build --workspace` must run before `cargo test --workspace`. `cargo test`
does not build `crates/wsharp-start`, which has no test target and which nothing
depends on, while the case suite's AOT pass links against whatever
`libwsharp_start.a` an earlier build left behind. A cold tree fails every built
case; a warm one silently links a stale runtime and **passes**, which is worse.

Paste the commands and their output into the issue. Paraphrase loses the detail
that turns out to matter.

## The weekly report

Posted weekly, on the triage pass. The format:

```markdown
## Defect report — week ending YYYY-MM-DD

| | P1 | P2 | P3 | P4 | Total |
|---|---|---|---|---|---|
| Open at start |  |  |  |  |  |
| Opened |  |  |  |  |  |
| Fixed |  |  |  |  |  |
| **Open now** |  |  |  |  |  |

**Untriaged:** N (target: 0)
**Open P1:** N — *list each one, with owner and next action*

**Load this week:** what was actually exercising the compiler — driver programs,
soak hours, platforms covered. A defect count means nothing without it.

**Trend:** is the defect rate falling under constant or rising load?

**Notes:** anything the numbers do not say.
```

The load line is the one that is tempting to skip and carries most of the
information. Zero defects in a week when nothing ran is not evidence of anything.
A falling defect rate under constant or rising load is the actual signal that
v1.0 is close; a flat rate means it is not, however small the absolute number is.
