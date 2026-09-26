#!/usr/bin/env bash
# The conformance gate for **rejections**: what W# refuses, and in what words.
#
# `tests/cases` already pins what the language accepts, and `parity.sh` pins
# that the three execution modes agree. Neither pins a refusal beyond a
# substring: `crates/wsharp-cli/tests/cases.rs` checks that each `// error:`
# line appears *somewhere* in stderr. That leaves three ways for the compiler to
# get worse without a single test going red:
#
#   1. **The span can rot.** `--> file:12:9` becoming `--> file:1:1` still
#      contains the message. A diagnostic that points at the wrong line is a
#      diagnostic a user cannot act on, and no substring check can see it.
#   2. **The `= help:` line can vanish.** Nothing in the corpus expects one
#      today -- `grep -c 'help:' tests/cases/*.ws` is 0 -- while
#      `crates/wsharp-sema/src/infer.rs` sets over forty of them. The most
#      carefully written part of the diagnostics is the part nothing tests.
#   3. **`check` and `run` can disagree.** They did:
#      `tests/cases/err_unpinned_generic.ws` documents `check` accepting a
#      program `run` rejected, because `check` stopped before monomorphisation.
#      The more careful-sounding verb was the more permissive one, and the
#      corpus could not see it because it only ever ran one of them.
#
# So this compares the **whole rendered diagnostic**, byte for byte, against a
# committed snapshot -- message, `-->` location, the source line, the caret
# columns, the secondary labels and the help line -- for every case in
# `tests/cases` whose header says it must be refused. And it runs both `check`
# and `run` and requires them to refuse identically.
#
# Verdicts, one per case:
#
#   PINNED     stderr matched the snapshot, and `check` and `run` agreed
#   CHANGED    stderr differed from the snapshot; the diff is kept
#   ACCEPTED   the case says `// error:` and the compiler compiled it
#   NOSPAN     the diagnostic named no `--> file:line:col`, or named a line
#              that is not in the case file
#   CHECKRUN   `check` and `run` refused it differently
#   UNBLESSED  no snapshot is committed for this case yet
#
# Exit status, on the `freeze-check.sh` convention:
#
#   0  every case PINNED -- the gate is met
#   1  at least one CHANGED / ACCEPTED / NOSPAN / CHECKRUN -- the gate is *not*
#      met, and each one is a defect to file
#   2  the harness could not run (no compiler, no `timeout`)
#   3  the only finding is UNBLESSED -- the gate cannot be *answered* yet
#
# 3 is separate from 1 on purpose. An empty snapshot set must never read as
# success: "nothing to compare, so nothing differed" is exactly how a gate
# passes for the wrong reason. Until the snapshots are blessed on a machine that
# can build the compiler, this exits 3 and says so.
#
# Usage:
#   tests/harness/conform.sh                 verify against the snapshots
#   tests/harness/conform.sh --bless         (re)write the snapshots from the
#                                            compiler under test
#   tests/harness/conform.sh --case NAME     one case, by file stem
#   tests/harness/conform.sh --gc-stress     add --gc-stress to the `run` leg
#
# Blessing is a commit like any other: the diff *is* the change to the
# language's refusals, which is the point of reviewing it.

set -uo pipefail

# shellcheck source=tests/harness/lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

EXPECTED="${WSHARP_CONFORM_EXPECTED:-$ROOT/tests/conformance/expected}"
OUT="$REPORTS/conformance"

bless=0
only=""
gc_stress=0
timeout_s=60

while [ $# -gt 0 ]; do
    case "$1" in
        --bless)      bless=1 ;;
        --case)       only="$2"; shift ;;
        --gc-stress)  gc_stress=1 ;;
        --timeout)    timeout_s="$2"; shift ;;
        -h|--help)    sed -n '2,70p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'; exit 0 ;;
        *)            echo "conform: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

require_compiler

mkdir -p "$OUT/diffs" "$EXPECTED"
TSV="$OUT/results.tsv"
: >"$TSV"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-conform-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# A snapshot must not carry anything that differs between two machines that are
# both right. Three things do:
#
#   - the path the case was given, which differs by checkout directory. The case
#     is invoked by a path relative to the repository root, so the rendered
#     `--> ` line already reads `tests/cases/foo.ws`; this only catches a
#     Windows runner rendering it with backslashes.
#   - temporary directories, for a case that names one.
#   - the collector's statistics line, if the environment turned it on.
#
# Everything else -- every column of every caret -- is compared. Anything added
# to this list is a thing the conformance gate stops checking, so the list is
# short and each entry says why it is here.
conform_normalise() {
    sed -e '/^W# gc: /d' \
        -e 's|\\|/|g' \
        -e 's|/tmp/[A-Za-z0-9_./-]*|<tmp>|g'
}

# Run one verb over one case and print its stderr, normalised. stdout is
# discarded: a refused program has none that matters, and a case that printed
# before being refused would not be a rejection case.
diagnose() {
    local verb="$1" file="$2"; shift 2
    "$TIMEOUT_BIN" "$timeout_s" "$WSHARP" "$verb" "$file" "$@" 2>&1 >/dev/null
    printf 'exit %s\n' "$?"
}

# Does the rendered diagnostic name a location, and is that location a line the
# case actually has? A span that rots to `1:1`, or past the end of the file, is
# the failure this catches -- the message is intact, so nothing else can.
span_is_sane() {
    local rendered="$1" file="$2" lines loc line
    loc="$(printf '%s\n' "$rendered" | sed -n 's|^ *--> \([^ ]*\):\([0-9]*\):\([0-9]*\)$|\2|p' | head -1)"
    [ -n "$loc" ] || return 1
    lines="$(wc -l <"$file" | tr -d ' ')"
    # A span may legitimately point one past the last line (end of file).
    [ "$loc" -ge 1 ] && [ "$loc" -le $((lines + 1)) ]
}

# Every `// error:` substring the Rust harness would require must still appear.
# Checked here too so a blessed snapshot cannot drift away from the case's own
# stated intent: the snapshot says what the compiler printed, the header says
# what the case is about, and the two must describe the same refusal.
header_substrings_present() {
    local file="$1" rendered="$2" want
    while IFS= read -r want; do
        [ -n "$want" ] || continue
        case "$rendered" in
            *"$want"*) ;;
            *) return 1 ;;
        esac
    done <<EOF
$(sed -n 's|^[[:space:]]*//[[:space:]]*error:[[:space:]]*||p' "$file")
EOF
    return 0
}

pinned=0; changed=0; accepted=0; nospan=0; checkrun=0; unblessed=0; blessed=0
helps=0; secondaries=0; total=0

run_flags=()
[ "$gc_stress" -eq 1 ] && run_flags+=(--gc-stress)

for file in "$CASES"/*.ws; do
    [ -e "$file" ] || continue
    name="$(basename "$file" .ws)"
    [ -z "$only" ] || [ "$only" = "$name" ] || continue
    # Only rejection cases. An accepted program's stdout is `parity.sh`'s gate.
    case_expects_error "$file" || continue
    total=$((total + 1))

    # Relative to the root, so the rendered `--> ` line is the same string on
    # every machine. `cd` into a subshell rather than changing the caller's
    # directory.
    rel="${file#"$ROOT"/}"
    check_out="$(cd "$ROOT" && diagnose check "$rel")"
    run_out="$(cd "$ROOT" && diagnose run "$rel" "${run_flags[@]+"${run_flags[@]}"}")"
    check_out="$(printf '%s\n' "$check_out" | conform_normalise)"
    run_out="$(printf '%s\n' "$run_out" | conform_normalise)"

    detail=""
    verdict=""

    # The compiler accepted a program the case says must be refused. Checked
    # before anything is compared: there is no diagnostic to snapshot, and a
    # blessed "no output" snapshot would pin the regression as correct.
    if ! printf '%s\n' "$check_out" | grep -q '^error'; then
        if ! printf '%s\n' "$run_out" | grep -q '^error'; then
            verdict=ACCEPTED
            detail="neither check nor run refused it"
        fi
    fi

    # `check` and `run` must refuse identically. This is the differential the
    # `check_and_run_agree_about_specialisation` regression came from, applied
    # to every rejection in the corpus rather than to one case.
    if [ -z "$verdict" ] && [ "$check_out" != "$run_out" ]; then
        verdict=CHECKRUN
        detail="check and run refused it differently"
        printf 'case: %s\n\n--- check ---\n%s\n\n--- run ---\n%s\n' \
            "$name" "$check_out" "$run_out" >"$OUT/diffs/$name.check-vs-run.txt"
    fi

    # `check` is the snapshot's subject: it is the diagnostic path, it needs no
    # backend, and it is what an editor would show.
    rendered="$check_out"
    snapshot="$EXPECTED/$name.diag"

    if [ -z "$verdict" ] && ! span_is_sane "$rendered" "$file"; then
        verdict=NOSPAN
        detail="no --> line, or it names a line $name does not have"
    fi

    if [ -z "$verdict" ] && ! header_substrings_present "$file" "$rendered"; then
        verdict=CHANGED
        detail="an // error: substring from the case header is missing"
        printf '%s\n' "$rendered" >"$OUT/diffs/$name.header.txt"
    fi

    if [ -z "$verdict" ] && [ "$bless" -eq 1 ]; then
        printf '%s\n' "$rendered" >"$snapshot"
        verdict=PINNED
        detail="blessed"
        blessed=$((blessed + 1))
    elif [ -z "$verdict" ]; then
        if [ ! -f "$snapshot" ]; then
            verdict=UNBLESSED
            detail="no snapshot at tests/conformance/expected/$name.diag"
            printf '%s\n' "$rendered" >"$OUT/diffs/$name.proposed.diag"
        else
            printf '%s\n' "$rendered" >"$WORK/got.diag"
            if diff -u "$snapshot" "$WORK/got.diag" >"$OUT/diffs/$name.diag.diff" 2>&1; then
                rm -f "$OUT/diffs/$name.diag.diff"
                verdict=PINNED
            else
                verdict=CHANGED
                detail="stderr differs from the snapshot; see diffs/$name.diag.diff"
            fi
        fi
    fi

    # Diagnostic *quality* coverage, reported rather than gated: how much of the
    # corpus pins a help line and a secondary label at all. A refusal with no
    # help line is not a defect, but a suite where none of them have one is not
    # testing the part of the diagnostics that took the most care to write.
    case "$rendered" in *"= help: "*) helps=$((helps + 1)) ;; esac
    if [ "$(printf '%s\n' "$rendered" | grep -c '^ *--> ')" -gt 0 ] &&
       [ "$(printf '%s\n' "$rendered" | grep -c '\^')" -gt 1 ]; then
        secondaries=$((secondaries + 1))
    fi

    case "$verdict" in
        PINNED)    pinned=$((pinned + 1)) ;;
        CHANGED)   changed=$((changed + 1)) ;;
        ACCEPTED)  accepted=$((accepted + 1)) ;;
        NOSPAN)    nospan=$((nospan + 1)) ;;
        CHECKRUN)  checkrun=$((checkrun + 1)) ;;
        UNBLESSED) unblessed=$((unblessed + 1)) ;;
    esac

    printf '%s\t%s\t%s\n' "$name" "$verdict" "$detail" >>"$TSV"
    printf '%-40s %s%s\n' "$name" "$verdict" "${detail:+  ($detail)}"
done

# ---------------------------------------------------------------------------
# The report. One row per case above; the totals and the provenance here.
# ---------------------------------------------------------------------------
{
    echo "# W# conformance: rejections and their diagnostics"
    echo
    echo "- compiler: \`$WSHARP\`"
    echo "- source SHA: $(source_sha)"
    echo "- last commit touching \`crates\`: $(compiler_source_sha)"
    echo "- platform: $(platform_line)"
    echo "- mode: check + run$([ "$gc_stress" -eq 1 ] && echo ' --gc-stress')"
    echo "- cases: $total rejection cases in $CASES"
    echo
    echo "| verdict | cases |"
    echo "|---|---|"
    echo "| PINNED | $pinned |"
    echo "| CHANGED | $changed |"
    echo "| ACCEPTED | $accepted |"
    echo "| NOSPAN | $nospan |"
    echo "| CHECKRUN | $checkrun |"
    echo "| UNBLESSED | $unblessed |"
    echo
    echo "Diagnostic shape across the corpus (reported, not gated):"
    echo
    echo "- carry a \`= help:\` line: $helps of $total"
    echo "- carry more than one underline (a secondary label): $secondaries of $total"
} >"$OUT/report.md"

echo
echo "conform: $total rejection cases; $pinned pinned, $changed changed, $accepted accepted," \
     "$nospan without a usable span, $checkrun check/run disagreements, $unblessed unblessed"
[ "$bless" -eq 1 ] && echo "conform: blessed $blessed snapshot(s) into ${EXPECTED#"$ROOT"/}"
echo "conform: report at ${OUT#"$ROOT"/}/report.md"

# Zero cases is not a met gate, it is a gate that ran over nothing -- a mistyped
# `--case`, a `CASES` pointing somewhere empty, or a corpus that stopped carrying
# rejection cases at all. Reported as 2, "could not run", for the same reason
# UNBLESSED is 3: the one thing this suite must never do is answer "met" on the
# strength of a comparison it did not make.
if [ "$total" -eq 0 ]; then
    echo "conform: no rejection cases in $CASES${only:+ matching --case $only}" >&2
    echo "conform: a gate that compared nothing has not been met" >&2
    exit 2
fi
if [ $((changed + accepted + nospan + checkrun)) -gt 0 ]; then
    echo "conform: the gate is NOT met -- each row above that is not PINNED is a defect to file" >&2
    exit 1
fi
if [ "$unblessed" -gt 0 ]; then
    echo "conform: $unblessed case(s) have no committed snapshot, so this gate cannot be answered." >&2
    echo "conform: run \`tests/harness/conform.sh --bless\` and commit tests/conformance/expected/." >&2
    echo "conform: proposed snapshots are under ${OUT#"$ROOT"/}/diffs/*.proposed.diag" >&2
    exit 3
fi
echo "conform: every rejection in the corpus is pinned to its exact diagnostic"
exit 0
