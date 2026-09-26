# Defects: intake, severity, and triage

How a defect gets from "this program did the wrong thing" to a merged fix with a
test that would have caught it.

**Severity is defined in
[`RELEASE-CRITERIA-1.0.md`](../RELEASE-CRITERIA-1.0.md#the-severity-scale) and
nowhere else**, including here. This document cites it. Two copies of a P1
definition is a gap between a freeze gate and a release gate, and that gap is
discovered at the worst possible moment.

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

Defined in
[`RELEASE-CRITERIA-1.0.md`](../RELEASE-CRITERIA-1.0.md#the-severity-scale). It is
not repeated here on purpose — a severity scale in two documents is a scale that
disagrees with itself eventually, and the disagreement surfaces during a freeze,
when it is most expensive.

What matters for intake:

- The **filer proposes** a severity on the report form; the **triage owner for
  the surface confirms or changes it**, within one working day.
- **An unconfirmed report counts at the severity the filer proposed.** So "no
  open P1" cannot be satisfied by leaving reports untriaged — which is the
  property that makes the gate mean anything.
- Ambiguity resolves **upward**. A defect that might be a silently wrong answer
  is P1 until somebody demonstrates it is not.
- Severity is decided by **what the defect does**, not by how hard it is to hit.
  Rarity is not a mitigation: a silently wrong answer that happens once a month
  is still a silently wrong answer.

Triage owners, from the same document:

| Surface | Owner |
|---|---|
| Compiler, runtime, stdlib, the collector | Mira (WLA-3, WLA-4) |
| Release pipeline, install, artefacts, digests, sharpie, the `.wsharp` ecosystem | Ash (WLA-7) |
| Raython and its sample application | Wren (WLA-8) |
| Harness, conformance, fuzzing, soak infrastructure | Dex (WLA-6) |
| Benchmarks, collector measurement, published numbers | Ridge (WLA-5) |

Disagreement about a severity goes to Johnny.

## Triage

**Weekly, and nothing sits untriaged for a week.** Every open issue leaves triage
with three things:

- a **severity** label (`P1`–`P3`), replacing `needs-triage`,
- an **owner**, as the GitHub assignee,
- a **next action**, written as a comment — not "investigate", but the actual next
  step: reduce it further, ask the reporter for the libc version, write the
  failing case, bisect between two named commits.

A defect nobody can reproduce is a triage task, not a fix task. Say so on the
issue and ask for the exact input, platform and mode. It stays open and keeps its
proposed severity; "cannot reproduce" is not a resolution while the reporter can
still reproduce it.

### Not every report is a defect

**`v1.0-limitation` is a triage outcome, and it is not the same as P3.** The
severity scale grades what a defect *does*, so it presumes there is one. A
report that describes something W# does not support — and refuses cleanly, or
never claimed — has no defect to grade, and forcing it onto the scale would
inflate every count with things working as designed. `P3` means a real defect
that gates nothing; `v1.0-limitation` means there is nothing wrong.

The test is what a supported program does, not how inconvenient the answer is:

- A **compile error with a correct message** for a program W# never claimed to
  accept is a limitation. P2 is "rejects a program it *should* accept", and the
  word doing the work is *should*.
- A **refusal at runtime**, with a diagnostic, of something outside the
  documented domain is a limitation. Refusing too much is a limitation;
  *accepting* too much is a defect, and in the crypto surface a P1.
- An **absent capability** — an architecture, a curve, a syntax — is a
  limitation whatever it costs the person who wanted it.

Two rules keep this from becoming a place to hide defects:

1. **It is a disposition, not a dismissal.** The issue stays open until the
   limitation is listed in `LIMITATIONS.md`, because the release criteria check
   that list against issue numbers. Closing one removes the thing the check
   asserts against.
2. **A limitation and a severity can coexist, and then the severity wins.** A
   P2 that is not being fixed for 1.0 carries both labels: `v1.0-limitation`
   records the decision, `P2` keeps it in the counts, and the criteria make the
   listing mandatory rather than editorial — an open P2 that is neither fixed
   nor listed becomes a P1, because it makes the documentation wrong.

Limitation-only issues are **excluded from the weekly counts** and reported as
their own line. Mixing them in would make "open P2" mean nothing.

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

Run the checks that prove *this* fix, not the whole suite by reflex.

**Always build the workspace first:**

```sh
cargo build --workspace
```

`cargo test` does not build `crates/wsharp-start` — it declares `test = false`
and nothing depends on it — while the case suite's AOT pass links against
whatever `libwsharp_start.a` an earlier build left in `target/debug`. A cold
tree fails every built case; a warm one silently links a stale runtime and
**passes**, which is worse.

**To check one case, run it, in each mode you care about:**

```sh
./target/debug/wsharp run             tests/cases/<case>.ws
./target/debug/wsharp run --gc-stress tests/cases/<case>.ws
./target/debug/wsharp build tests/cases/<case>.ws -o /tmp/<case> && /tmp/<case>
```

There is **no way to run a single case through `cargo test`**, and the way that
looks like it should work is actively dangerous:

```console
$ cargo test -p wsharp-cli --test cases arithmetic
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 15 filtered out
```

A test filter matches the names of *test functions*, and the case suite has no
function per case — it has three functions that each iterate every case. So a
case name filters everything out and reports `ok`. Green, and it ran nothing.
This is the same trap as the stale archive above, which is why both are written
down here.

**To run the suite, filter by the harness function:**

```sh
cargo test -p wsharp-cli --test cases every_case_behaves_as_declared               # JIT
cargo test -p wsharp-cli --test cases every_case_survives_collecting_at_every_allocation
cargo test -p wsharp-cli --test cases every_case_behaves_the_same_built_as_run     # AOT
cargo test -p wsharp-<crate>                                                       # unit tests
```

Touched the runtime or the collector? The `--gc-stress` pass above is the one
that matters, plus `the_collector_survives_stress_in_a_built_program`.

One more trap, because it costs a confusing five minutes: **`cargo test
--workspace <filter>` fails to compile.** Given a filter argument, cargo builds
`wsharp-start` with `--test` despite its `test = false`, and that crate defines
`main`, so the linker is asked to place two and refuses:

```text
error: entry symbol `main` declared multiple times
```

Nothing is wrong with your change. Use `-p wsharp-cli --test cases` as above, or
`--workspace --exclude wsharp-start`. Plain `cargo test --workspace`, with no
filter, is fine and is what CI runs.

Paste the commands and their output into the issue. Paraphrase loses the detail
that turns out to matter.

## The weekly report

Posted weekly, on the triage pass. The format:

```markdown
## Defect report — week ending YYYY-MM-DD

| | P1 | P2 | P3 | Total |
|---|---|---|---|---|
| Open at start |  |  |  |  |
| Opened |  |  |  |  |
| Fixed |  |  |  |  |
| **Open now** |  |  |  |  |

**Untriaged:** N (target: 0)
**Open P1:** N — *list each one, with owner and next action*
**Tracked limitations:** N open — *not defects, counted separately*

**Load this week:** what was actually exercising the compiler — driver programs,
soak hours, platforms covered. A defect count means nothing without it.

**Trend:** is the defect rate falling under constant or rising load?

**Notes:** anything the numbers do not say.
```

The load line is the one that is tempting to skip and carries most of the
information. Zero defects in a week when nothing ran is not evidence of anything.
A falling defect rate under constant or rising load is the actual signal that
v1.0 is close; a flat rate means it is not, however small the absolute number is.
