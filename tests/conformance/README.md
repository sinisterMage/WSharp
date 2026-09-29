# The conformance suite: what W# refuses, and in what words

`tests/cases` says what W# accepts. `tests/harness/parity.sh` says that the
three execution modes agree about it. This directory holds the other half of
criterion 5: the **exact diagnostic** for every program the language refuses.

One command, from a clean checkout, on any of the four release triples:

```sh
cargo build --workspace          # the compiler and the runtime archive
tests/harness/conform.sh
```

And the selftest, which needs no cargo, no `cc` and no W# compiler at all:

```sh
tests/harness/conform-selftest.sh
```

## Why a snapshot rather than a substring

`crates/wsharp-cli/tests/cases.rs` already checks that each `// error:` line in
a case's header appears *somewhere* in stderr. That leaves three ways for the
compiler's diagnostics to get worse with nothing going red.

**The span can rot.** `--> tests/cases/foo.ws:12:9` becoming `--> …:1:1` still
contains the message, so every substring check still passes. A diagnostic that
points at the wrong line is one a user cannot act on, and spans are computed
from a byte offset and a line index — arithmetic, which is exactly the kind of
thing that breaks quietly.

**The `= help:` line can vanish.** `grep -c 'help:' tests/cases/*.ws` is **0**,
while `crates/wsharp-sema/src/infer.rs` sets over forty of them. CLAUDE.md's
style rule says the message states the rule and the help says what to do — and
the half that says what to do is the half nothing tests. Every one of those help
lines could be deleted today and the suite would stay green.

**`check` and `run` can disagree.** They did.
`tests/cases/err_unpinned_generic.ws` documents it: `wsharp check` stopped before
monomorphisation and so *accepted* a program `wsharp run` refused, which made
the more careful-sounding verb the more permissive one. The corpus could not see
it, because each case was only ever run one way.

So `conform.sh` compares the whole rendered diagnostic, byte for byte, against a
committed snapshot in `expected/`: the message, the `-->` location, the quoted
source line, the caret columns, the secondary labels and the help line. And it
runs both `check` and `run` over every rejection case and requires them to refuse
identically.

## Verdicts

One row per rejection case in `target/harness/conformance/results.tsv`, and a
report beside it naming the compiler, the commit, the platform and the totals.

| Verdict | Means | Who owns it |
|---|---|---|
| `PINNED` | stderr matched the snapshot; `check` and `run` agreed | nobody, this is the gate met |
| `CHANGED` | stderr differs from the snapshot | read the diff: either a diagnostic regressed (a defect for Mira) or it improved (a bless commit) |
| `ACCEPTED` | the case says `// error:` and the compiler compiled it | a defect, and a severe one: the compiler started accepting something the language refuses |
| `NOSPAN` | no `--> file:line:col`, or it names a line the case does not have | a defect |
| `CHECKRUN` | `check` and `run` refused it differently | a defect, and the `err_unpinned_generic` class |
| `UNBLESSED` | no snapshot committed for this case | run `--bless` and commit |

Exit status follows `scripts/freeze-check.sh`'s convention:

- `0` — every case `PINNED`. The gate is met.
- `1` — at least one `CHANGED` / `ACCEPTED` / `NOSPAN` / `CHECKRUN`. The gate is
  **not** met, and each such row is a defect to file with the reproducer the
  report already contains.
- `2` — the harness could not run: no compiler, no `timeout`, **or zero
  rejection cases matched**. A gate that compared nothing has not been met, and
  that hole was real — the first version of `conform.sh` printed "every rejection
  in the corpus is pinned" and exited 0 for a mistyped `--case`.
- `3` — the only finding is `UNBLESSED`. The gate cannot be **answered**.

3 is a separate status from 1 deliberately. "Nothing to compare, so nothing
differed" is precisely how a gate passes for the wrong reason, and an empty
snapshot set must never read as success. `conform-selftest.sh` asserts that
exit 3 rather than 0 is what an unsnapshotted corpus produces.

## Blessing

```sh
tests/harness/conform.sh --bless
git add tests/conformance/expected
```

The diff *is* the change to the language's refusals, which is the point of
reviewing it rather than regenerating it in CI. Two things `--bless` will not do:
it writes no snapshot for a case it reported as `ACCEPTED`, `NOSPAN` or
`CHECKRUN` — those are defects, and pinning a defect as correct is worse than
leaving the case unblessed — and it refuses a case whose snapshot and whose
`// error:` header no longer describe the same refusal, because that is the one
finding a reviewer skimming a bless diff would not notice.

**`expected/` holds 63 snapshots, blessed from a real `conformance` job's
artifact, not from a local build.** Nobody on this campaign has a machine that
can build W# — no `rustc`, no `cargo`, no `cc` — so guessed diagnostics would
have produced failures on the first real run that looked like compiler defects
and were typing mistakes. What happened instead: the run of the `conformance`
job (dispatched manually on 2026-09-27, run `36296023948`) exited 3 and uploaded
every diagnostic it *would* have pinned as
`conformance-<triple>/diffs/*.proposed.diag`; the snapshots here are those
bytes, normalised (see below) and committed. They came from four independent
triples and, after normalisation, were **byte-identical on all four** — which is
itself the first evidence that a diagnostic is a platform-independent claim.
The sixty-third (`err_const_string_array`) was added when `main` gained that
case (#58) while the bless PR was open; the scheduled run `36306746001`
proposed it and re-confirmed the other sixty-two with zero mismatches.

The four `err_ffi_*` cases were left un-blessed while they reported `NOSPAN`
(their `-->` named a line of `std/ffi`, not of the user's file --
[#37](https://github.com/sinisterMage/WSharp/issues/37)), deliberately, because
pinning a diagnostic that points at the wrong file would pin the defect as
correct. They were blessed with the fix that points them at the program's own
`ffi.bind` call, so every rejection case is now pinned. The compiler subject's
soak row reads `fail` on any night a case is `NOSPAN` again -- because criterion
5 genuinely is not met while that is true.

## What this suite does not cover

Written here rather than left in anyone's head.

- **The four `err_ffi_*` cases are `NOSPAN` and un-blessed.** Their `-->` names
  a line of `std/ffi`, not of the user's file, so there is no span to pin; this
  is [#37](https://github.com/sinisterMage/WSharp/issues/37) and the gate is
  correctly red while it stands.
- **Only the 68 cases in `tests/cases` whose header carries `// error:`.** It
  adds no new rejection cases. The gaps in what the language refuses are still
  gaps; this pins the refusals that exist.
- **`check` is the snapshot's subject.** `run`'s stderr is compared with
  `check`'s but is not itself snapshotted, so a diagnostic only `run` can produce
  is pinned as "identical to check" rather than in its own right.
- **`build` is not in this gate.** A rejection is refused before code generation,
  so `wsharp build` has nothing to add; if that ever stops being true, this is
  the sentence that was wrong.
- **Warnings are not pinned.** `Severity::Warning` exists and no case expects
  one, so nothing here would notice a warning appearing or disappearing.
- **The `// panic:` cases are not here.** A runtime panic is not a diagnostic;
  it is stderr from a program that compiled, and `cases.rs` holds it to its
  substring. Pinning panic text is a separate piece of work.
- **Four things are normalised away** and are therefore not checked: a
  `W# gc:` statistics line, `/tmp/...` paths, `\` rewritten to `/` so the
  Windows runner compares against the same snapshot, and the C library's
  `strerror` words in an `(os error N)`. The errno is kept — `ENOENT` against
  `EACCES` still differs — but the sentence in front of it is the platform's,
  and the first real run had the four triples disagree on nothing but that
  string (`err_import_missing_file`: Linux says "No such file or directory",
  Windows says "The system cannot find the file specified."). Nothing else is
  normalised — every caret column is compared.
- **`--gc-stress` changes nothing here and is accepted anyway.** A program
  refused at compile time never allocates. The flag is plumbed through so that
  if a diagnostic ever *did* depend on it, the run that noticed would be this
  one.
- **No multi-file rejection is covered.** Every case is a single file, so a
  diagnostic whose primary span is in one module and whose secondary label is in
  another — which `SourceMap` exists to render — is unexercised.
- **A snapshot is not a judgement.** `PINNED` means the diagnostic has not
  changed. It does not mean it is good, that the span is on the most useful
  token, or that the help line would help.
