#!/usr/bin/env bash
# Can `conform.sh` tell a rotted diagnostic from an intact one?
#
# `conform.sh` gates the language's refusals by comparing whole rendered
# diagnostics against committed snapshots. Its failure mode is the same one
# `parity.sh` has and for the same reason: a comparison that has stopped
# comparing reports nothing to report, and an empty findings list is what the
# gate reads as success. The specific ways it could pass for the wrong reason:
#
#   - it compares only the message and so cannot see a moved span,
#   - it drops the `= help:` line in normalisation and so cannot see it vanish,
#   - it treats a missing snapshot as "nothing differed" instead of "cannot
#     answer",
#   - it runs only `check`, and so cannot see `check` and `run` disagree.
#
# So this builds a **stub compiler** that prints diagnostics dictated by the case
# file, hands `conform.sh` a corpus whose verdict is known in advance for each
# one, and asserts every verdict, the `--bless` round trip, and all three exit
# statuses. What is under test is the gate, not W#.
#
# It needs no cargo, no `cc` and no W# compiler: it runs in the per-PR job in
# seconds, and on a machine that cannot build the language at all. A change that
# breaks the gate's verdicts then fails on the pull request that made it rather
# than surviving until a nightly whose findings list is empty for the wrong
# reason.
#
# What it does **not** test: whether W#'s real diagnostics are good, whether its
# spans are right, or whether any snapshot in `tests/conformance/expected` is
# correct. Those are what a real `conform.sh` run answers, and this cannot stand
# in for one.
#
# Usage:
#   tests/harness/conform-selftest.sh

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-conform-selftest-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

failures=0
ok()  { printf 'ok    %s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
# The stub compiler.
#
# Accepts `check FILE` and `run [--gc-stress] FILE` and prints, on stderr, the
# case file's own `//>` lines with that prefix removed -- so a stub case carries
# the exact bytes it wants the gate to see. `//R>` lines, when present, are what
# `run` prints instead, which is how a check/run disagreement is written.
#
#   //@accept     print nothing and exit 0, as a compiler that took the program
#                 would
#
# Exits 1 whenever it printed a diagnostic, as the real driver does.
# ---------------------------------------------------------------------------
cat >"$WORK/wsharp" <<'STUB'
#!/usr/bin/env bash
# Stub compiler for tests/harness/conform-selftest.sh. Not W#.
set -uo pipefail
verb="$1"; shift
[ "${1:-}" = "--gc-stress" ] && shift
file="$1"

grep -q '^//@accept' "$file" && exit 0

prefix='//>'
if [ "$verb" = run ] && grep -q '^//R>' "$file"; then
    prefix='//R>'
fi
# `sed` rather than a read loop: a diagnostic's gutter lines (`  |`) must
# survive verbatim, trailing spaces and all. Two expressions rather than one
# alternation, and `@` for the delimiter: a `|` delimiter would read the `\|` of
# a BRE alternation as an escaped delimiter and match nothing -- which is a
# stub that prints no diagnostic, which every case then reports as ACCEPTED.
sed -n -e "s@^${prefix} @@p" -e "s@^${prefix}\$@@p" "$file" >&2
exit 1
STUB
chmod +x "$WORK/wsharp"

CASES="$WORK/cases"
SNAPS="$WORK/expected"
mkdir -p "$CASES" "$SNAPS"

# --- the corpus, one case per verdict --------------------------------------

# Refused exactly as the snapshot says. The baseline: if this is not PINNED the
# gate cannot pass at all.
cat >"$CASES/pinned.ws" <<'EOF'
// error: `a` is declared more than once
//> error: `a` is declared more than once
//>   --> cases/pinned.ws:2:7
//>   |
//> 2 | const a = 2;
//>   |       ^
//>   |
//>   = help: pick another name
EOF

# The message is intact and the span has moved from column 7 to column 11. A
# substring check cannot see this; it is the whole reason the snapshot is
# compared byte for byte.
cat >"$CASES/moved_span.ws" <<'EOF'
// error: `a` is declared more than once
//> error: `a` is declared more than once
//>   --> cases/moved_span.ws:2:11
//>   |
//> 2 | const a = 2;
//>   |           ^
EOF

# The message and the span are intact and the `= help:` line is gone. Forty-odd
# help lines in `wsharp-sema` and nothing in `tests/cases` expects one, so this
# is the regression the corpus is blindest to.
cat >"$CASES/lost_help.ws" <<'EOF'
// error: unknown supertype `Shape`
//> error: unknown supertype `Shape`
//>   --> cases/lost_help.ws:1:1
//>   |
//> 1 | // error: unknown supertype `Shape`
//>   | ^
EOF

# The compiler took a program the case says must be refused.
cat >"$CASES/accepted.ws" <<'EOF'
// error: this must not compile
//@accept
EOF

# A diagnostic with no location at all -- what `diag.rs::render` falls back to
# when a span points at no file. The message is right and a user cannot act on
# it.
cat >"$CASES/nospan.ws" <<'EOF'
// error: cannot tell what type `main` is being used at
//> error: cannot tell what type `main` is being used at
EOF

# A span that survived but rotted past the end of the file: line 900 of a
# six-line case. `1:1` is the other common shape and is indistinguishable from a
# real diagnostic on line 1, which is why the check is "a line this file has"
# rather than "not 1:1".
cat >"$CASES/span_past_eof.ws" <<'EOF'
// error: unexpected token
//> error: unexpected token
//>   --> cases/span_past_eof.ws:900:3
//>   |
//> 900 |
//>   | ^
EOF

# `check` accepts the shape of the refusal that `run` reports differently. The
# historical bug: `check` stopped before monomorphisation.
cat >"$CASES/checkrun.ws" <<'EOF'
// error: cannot tell what type `main` is being used at
//> error: cannot tell what type `main` is being used at
//>   --> cases/checkrun.ws:4:13
//>   |
//> 4 |     var b = array.new(32);
//>   |             ^
//R> error: cannot tell what type `main` is being used at
//R>   --> cases/checkrun.ws:1:1
//R>   |
//R> 1 | // error: cannot tell what type `main` is being used at
//R>   | ^
EOF

# Refused, sanely, and never blessed. Must be "cannot answer", not "agreed".
cat >"$CASES/unblessed.ws" <<'EOF'
// error: a supertype must be a struct
//> error: a supertype must be a struct
//>   --> cases/unblessed.ws:1:4
//>   |
//> 1 | // error: a supertype must be a struct
//>   |    ^
EOF

# The snapshot and the case header no longer describe the same refusal: the
# compiler now says something else entirely and someone blessed it without
# reading the case. The header is the case's stated intent and outranks the
# snapshot.
# The snippet line quotes line 3 rather than the header, because a snippet that
# echoed the header would contain the substring being looked for and the case
# would pass for the wrong reason.
cat >"$CASES/header_drift.ws" <<'EOF'
// error: `error.Zero` is not one of `{Negative}`
//> error: something completely different
//>   --> cases/header_drift.ws:3:5
//>   |
//> 3 |     return n;
//>   |     ^
EOF

# An accepted program. It must be skipped entirely: its gate is `parity.sh`.
cat >"$CASES/not_a_rejection.ws" <<'EOF'
// expect: 7
//@accept
EOF

# --- bless the snapshots the verify run expects to find --------------------
#
# Written from the stub's own output for the cases that must come out PINNED,
# and deliberately *stale* for the two that must come out CHANGED.
bless_one() {
    local name="$1"
    (cd "$WORK" && "$WORK/wsharp" check "cases/$name.ws" 2>&1 >/dev/null; printf 'exit %s\n' "$?") \
        >"$SNAPS/$name.diag"
}
bless_one pinned
bless_one header_drift

# `moved_span` is snapshotted at the column it used to point at, and `lost_help`
# with the help line it used to print. The stub now prints neither.
cat >"$SNAPS/moved_span.diag" <<'EOF'
error: `a` is declared more than once
  --> cases/moved_span.ws:2:7
  |
2 | const a = 2;
  |       ^
exit 1
EOF
cat >"$SNAPS/lost_help.diag" <<'EOF'
error: unknown supertype `Shape`
  --> cases/lost_help.ws:1:1
  |
1 | // error: unknown supertype `Shape`
  | ^
  |
  = help: a supertype must be declared before it is named
exit 1
EOF

# --- run the real conform.sh over it --------------------------------------
REPORT="$WORK/report"
run_conform() {
    WSHARP="$WORK/wsharp" CASES="$1" \
        WSHARP_CONFORM_EXPECTED="$2" WSHARP_HARNESS_REPORTS="$3" \
        bash "$HERE/conform.sh" --timeout 30 "${@:4}"
}

run_conform "$CASES" "$SNAPS" "$REPORT" >"$WORK/conform.log" 2>&1
status=$?

TSV="$REPORT/conformance/results.tsv"
if [ ! -f "$TSV" ]; then
    echo "selftest: conform.sh wrote no results.tsv; its output was:" >&2
    cat "$WORK/conform.log" >&2
    exit 1
fi

verdict() { awk -F'\t' -v c="$1" '$1==c {print $2}' "$TSV"; }

expect() {
    local name="$1" want="$2" got
    got="$(verdict "$name")"
    if [ "$got" = "$want" ]; then
        ok "$name -> $want"
    else
        bad "$name -> got '${got:-<no row>}', wanted '$want'"
    fi
}

expect pinned        PINNED
expect moved_span    CHANGED
expect lost_help     CHANGED
expect accepted      ACCEPTED
expect nospan        NOSPAN
expect span_past_eof NOSPAN
expect checkrun      CHECKRUN
expect unblessed     UNBLESSED
expect header_drift  CHANGED

# An accepted program is not this gate's business, and a row for it would make
# the corpus count wrong in both reports.
if [ -z "$(verdict not_a_rejection)" ]; then
    ok "an accepted program is left to parity.sh, not reported here"
else
    bad "not_a_rejection got a row ('$(verdict not_a_rejection)'); accepted programs are parity.sh's"
fi

# A CHANGED verdict a reader cannot act on costs the fixer the reduction, so the
# kept diff is part of the contract.
if grep -q '2:7' "$REPORT/conformance/diffs/moved_span.diag.diff" 2>/dev/null &&
   grep -q '2:11' "$REPORT/conformance/diffs/moved_span.diag.diff" 2>/dev/null; then
    ok "moved_span kept a diff naming both the old and the new column"
else
    bad "moved_span kept no usable diff under $REPORT/conformance/diffs/"
fi
if grep -q 'help' "$REPORT/conformance/diffs/lost_help.diag.diff" 2>/dev/null; then
    ok "lost_help kept a diff naming the help line"
else
    bad "lost_help kept no diff naming the help line"
fi
if [ -s "$REPORT/conformance/diffs/checkrun.check-vs-run.txt" ]; then
    ok "checkrun kept both refusals side by side"
else
    bad "checkrun kept nothing to compare"
fi
# An unblessed case must hand over the bytes to bless, or the fix is a rerun on
# a machine the reader may not have.
if [ -s "$REPORT/conformance/diffs/unblessed.proposed.diag" ]; then
    ok "unblessed left a proposed snapshot to review"
else
    bad "unblessed left no proposed snapshot"
fi

# --- exit statuses --------------------------------------------------------
if [ "$status" -eq 1 ]; then
    ok "a corpus with real findings exits 1 (was $status)"
else
    bad "wanted exit 1 for a corpus with CHANGED/ACCEPTED/NOSPAN/CHECKRUN, got $status"
fi

# Unblessed alone is "cannot answer" (3), never "met" (0). This is the
# assertion that stops an empty snapshot set from reading as a green gate.
ONLY_UNBLESSED="$WORK/only-unblessed"; mkdir -p "$ONLY_UNBLESSED"
cp "$CASES/unblessed.ws" "$ONLY_UNBLESSED/"
run_conform "$ONLY_UNBLESSED" "$WORK/empty-snaps" "$WORK/report-unblessed" \
    >"$WORK/conform-unblessed.log" 2>&1
if [ $? -eq 3 ]; then
    ok "a corpus with no snapshots exits 3 (cannot answer), not 0"
else
    bad "wanted exit 3 for an unblessed corpus, got $? -- an empty snapshot set read as success"
fi

# A corpus with no rejection cases in it at all -- a mistyped `--case`, a `CASES`
# pointing at the wrong directory -- must not be "met". This one was a real hole:
# the first version of `conform.sh` printed "every rejection in the corpus is
# pinned" and exited 0 for `--case nosuch`, which is a green gate over nothing.
run_conform "$CASES" "$SNAPS" "$WORK/report-nocases" --case nosuchcase \
    >"$WORK/conform-nocases.log" 2>&1
if [ $? -eq 2 ]; then
    ok "a run that matched no case exits 2, not 0"
else
    bad "a run over zero cases exited $? -- a gate that compared nothing read as met"
fi

# And the inverse: a corpus that is fully pinned must exit 0, or the gate can
# never be met.
CLEAN="$WORK/clean"; mkdir -p "$CLEAN"
cp "$CASES/pinned.ws" "$CLEAN/"
run_conform "$CLEAN" "$SNAPS" "$WORK/report-clean" >"$WORK/conform-clean.log" 2>&1
if [ $? -eq 0 ]; then
    ok "a fully pinned corpus exits 0"
else
    bad "a fully pinned corpus did not exit 0"
    cat "$WORK/conform-clean.log" >&2
fi

# --- the bless round trip -------------------------------------------------
#
# `--bless` writing something a following verify run rejects would make the
# snapshots unmaintainable: every bless commit would arrive red.
BLESSABLE="$WORK/blessable"; mkdir -p "$BLESSABLE"
cp "$CASES/pinned.ws" "$CASES/unblessed.ws" "$CASES/lost_help.ws" "$BLESSABLE/"
FRESH="$WORK/fresh-snaps"
run_conform "$BLESSABLE" "$FRESH" "$WORK/report-bless" --bless >"$WORK/bless.log" 2>&1
bless_status=$?
run_conform "$BLESSABLE" "$FRESH" "$WORK/report-reverify" >"$WORK/reverify.log" 2>&1
reverify_status=$?
if [ "$bless_status" -eq 0 ] && [ "$reverify_status" -eq 0 ]; then
    ok "--bless then verify round-trips to exit 0"
else
    bad "bless exited $bless_status, re-verify exited $reverify_status; wanted 0 and 0"
    cat "$WORK/bless.log" "$WORK/reverify.log" >&2
fi
if [ -f "$FRESH/unblessed.diag" ] && grep -q -- '--> cases/unblessed.ws:1:4' "$FRESH/unblessed.diag"; then
    ok "--bless wrote the whole diagnostic, span included"
else
    bad "--bless wrote no usable snapshot for unblessed"
fi
# Blessing must not paper over a case whose header and diagnostic disagree: that
# is the one finding a reviewer of a bless diff would not notice.
run_conform "$CASES" "$WORK/bless-all-snaps" "$WORK/report-bless-all" --bless \
    >"$WORK/bless-all.log" 2>&1
bless_all_status=$?
bless_all_tsv="$WORK/report-bless-all/conformance/results.tsv"
drift="$(awk -F'\t' '$1=="header_drift" {print $2}' "$bless_all_tsv" 2>/dev/null)"
if [ "$drift" = CHANGED ] && [ "$bless_all_status" -ne 0 ]; then
    ok "--bless still refuses a case whose header and diagnostic disagree"
else
    bad "--bless reported header_drift as '$drift' and exited $bless_all_status; wanted CHANGED and non-zero"
fi
if [ -f "$WORK/bless-all-snaps/header_drift.diag" ]; then
    bad "--bless wrote a snapshot for a case it reported as CHANGED"
else
    ok "--bless wrote no snapshot for the case it refused"
fi

# --- the report is the evidence ------------------------------------------
REPORT_MD="$REPORT/conformance/report.md"
if grep -q 'platform:' "$REPORT_MD" && grep -q 'source SHA:' "$REPORT_MD"; then
    ok "the report names the platform and the commit"
else
    bad "the report does not name the platform and the commit"
fi
if grep -q '= help' "$REPORT_MD" || grep -q 'help:.*of' "$REPORT_MD"; then
    ok "the report counts how much of the corpus pins a help line"
else
    bad "the report does not count help lines"
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "conform-selftest: the gate tells a rotted diagnostic from an intact one"
    exit 0
fi
echo "conform-selftest: $failures assertion(s) failed" >&2
exit 1
