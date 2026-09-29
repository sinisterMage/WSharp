#!/usr/bin/env bash
# Differential test: every case in `tests/cases`, in three execution modes,
# compared against *each other* rather than against its header.
#
# The suite in `crates/wsharp-cli/tests/cases.rs` already runs the corpus three
# ways and holds each run to the header comment. That catches a mode that gets
# the answer wrong. It cannot catch a mode that gets the answer right in a way
# the header does not describe -- a case whose header says nothing about stderr
# ordering, or one where two modes print the same lines for different reasons.
# Two paths that must agree turn "is this right?" into a mechanical check, so
# this compares the modes to one another, per case, and reports the diff.
#
#   run     the JIT:                wsharp run CASE
#   stress  the JIT, collecting at
#           every allocation:       wsharp run --gc-stress CASE
#   build   the object backend:     wsharp build CASE -o EXE && EXE
#
# **Determinism is established before parity is claimed, but only where it is
# in doubt.** A case that disagrees with itself is a finding about determinism,
# not about the backends, and reporting it as a divergence would bury the real
# ones. Establishing it up front costs two runs of every mode of every case,
# which on one core is hours; so each mode runs once, and a *divergence* is what
# triggers the re-runs that decide whether it is a difference between the modes
# or a case that does not agree with itself. The conclusion is the same and the
# cost falls on the handful of cases that have something to say.
#
# Usage:
#   tests/harness/parity.sh [--only PATTERN] [--timeout SECONDS] [--jobs N]
#
# Output lands in $WSHARP_HARNESS_REPORTS (default target/harness/parity-<stamp>):
#   results.tsv   one row per case: name, verdict, detail
#   report.md     the readable summary
#   diffs/        the actual outputs for every case that diverged

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ONLY=""
TIMEOUT=60
BUILD_TIMEOUT=180
JOBS=1

while [ $# -gt 0 ]; do
    case "$1" in
        --only) ONLY="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --jobs) JOBS="$2"; shift 2 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "parity: unknown argument $1" >&2; exit 2 ;;
    esac
done

require_compiler

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
# Pinned before anything runs: the checkout can move under a run of this length,
# and a report naming the SHA it finished at did not test that SHA.
SHA_AT_START="$(source_sha)"
COMPILER_SHA_AT_START="$(compiler_source_sha)"
OUT="${WSHARP_HARNESS_REPORTS:-$REPORTS/parity-$STAMP}"
mkdir -p "$OUT/diffs"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-parity-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# Run one case in one mode, into $WORK/<tag>.{out,err,code}, and the bound it
# ran under into $WORK/<tag>.bound -- the case's own `// timeout:` if it named
# one, the default otherwise, the compiler's for a build that never produced a
# program -- so a timeout row says the number that was tested against.
#
# `build` compiles first; a compile failure is the observable outcome and is
# recorded as such rather than as a harness error, because a case that must not
# compile has to fail identically in every mode.
execute() {
    local case_file="$1" mode="$2" tag="$3"
    local args; args="$(case_args "$case_file")"
    # A case may name its own bound; see `case_timeout` in `lib.sh` for why one
    # global number is wrong for both a hang and an expensive case.
    local bound; bound="$(case_timeout "$case_file")"
    bound="${bound:-$TIMEOUT}"
    local code
    case "$mode" in
        run)
            # shellcheck disable=SC2086
            "$TIMEOUT_BIN" "$bound" "$WSHARP" run "$case_file" $args \
                >"$WORK/$tag.out" 2>"$WORK/$tag.err"
            code=$?
            echo "${bound}s" >"$WORK/$tag.bound"
            ;;
        stress)
            # shellcheck disable=SC2086
            "$TIMEOUT_BIN" "$bound" "$WSHARP" run --gc-stress "$case_file" $args \
                >"$WORK/$tag.out" 2>"$WORK/$tag.err"
            code=$?
            echo "${bound}s" >"$WORK/$tag.bound"
            ;;
        build)
            local exe="$WORK/$tag.exe"
            # `if ! cmd` would report the negation's status rather than the
            # command's, which records a failed build as exit 0 and makes every
            # `err_*` case look like a divergence. Capture it plainly.
            "$TIMEOUT_BIN" "$BUILD_TIMEOUT" "$WSHARP" build "$case_file" -o "$exe" \
                >"$WORK/$tag.out" 2>"$WORK/$tag.err"
            code=$?
            if [ "$code" -ne 0 ]; then
                # The bound that fired, if one did, was the compiler's.
                echo "compiling, ${BUILD_TIMEOUT}s" >"$WORK/$tag.bound"
                echo "$code" >"$WORK/$tag.code"
                return 0
            fi
            # A successful build prints nothing the program did; start clean.
            : >"$WORK/$tag.out"; : >"$WORK/$tag.err"
            # shellcheck disable=SC2086
            "$TIMEOUT_BIN" "$bound" "$exe" $args \
                >"$WORK/$tag.out" 2>"$WORK/$tag.err"
            code=$?
            echo "${bound}s" >"$WORK/$tag.bound"
            rm -f "$exe"
            ;;
    esac
    echo "$code" >"$WORK/$tag.code"
}

# Did the harness's own bound fire on this run, rather than the program exiting?
#
# Every mode here runs under `timeout`, which answers 124 when its bound fires.
# That number is the harness's, not the program's: reporting it as an exit
# status makes a mode that ran out of time look like a mode that disagreed,
# which criterion 5 clause 1 routes straight to a P1 -- so the two are separated
# here, before the comparison, rather than papered over after it.
#
# **124 only.** `timeout` runs without `-k`, so it never sends KILL itself, and
# a 137 is a SIGKILL from somebody else -- the OOM killer, most likely. That is
# a crash of the mode, and it is compared like any other exit status.
is_timeout() {
    [ "$(cat "$WORK/$1.code")" = 124 ]
}

# Are two recorded outcomes observably the same? Exit status, then stdout,
# then stderr, each after `normalise`.
same() {
    local a="$1" b="$2"
    [ "$(cat "$WORK/$a.code")" = "$(cat "$WORK/$b.code")" ] || { echo "exit status"; return 1; }
    diff -q <(normalise <"$WORK/$a.out") <(normalise <"$WORK/$b.out") >/dev/null || { echo "stdout"; return 1; }
    diff -q <(normalise <"$WORK/$a.err") <(normalise <"$WORK/$b.err") >/dev/null || { echo "stderr"; return 1; }
    return 0
}

record_diff() {
    local name="$1" what="$2" a="$3" b="$4"
    {
        echo "### $name — $what"
        echo
        echo "exit: $a=$(cat "$WORK/$a.code")  $b=$(cat "$WORK/$b.code")"
        echo
        echo "--- $a stdout"; normalise <"$WORK/$a.out" | head -40
        echo "--- $b stdout"; normalise <"$WORK/$b.out" | head -40
        echo "--- $a stderr"; normalise <"$WORK/$a.err" | head -40
        echo "--- $b stderr"; normalise <"$WORK/$b.err" | head -40
    } >"$OUT/diffs/$name.$what.txt"
}

# A timeout is not a divergence between two modes, so the two-mode diff above
# would name the wrong pair: `run.a` against `stress.a` when it was `build` that
# ran out. Record every mode's status and output instead, and say which bound
# each was given.
record_timeout() {
    local name="$1" modes="$2" mode
    {
        echo "### $name — timeout"
        echo
        echo "Did not finish inside its bound. An exit code of 124 below is"
        echo "\`timeout\`'s, not the program's."
        echo
        echo "modes that timed out:$modes"
        echo
        for mode in run stress build; do
            echo "--- $mode exit: $(cat "$WORK/$mode.a.code")"
            echo "--- $mode stdout"; normalise <"$WORK/$mode.a.out" | head -20
            echo "--- $mode stderr"; normalise <"$WORK/$mode.a.err" | head -20
        done
    } >"$OUT/diffs/$name.timeout.txt"
}

TSV="$OUT/results.tsv"
printf 'case\tverdict\tseconds\tdetail\n' >"$TSV"

started="$(date +%s)"
total=0; agreed=0; diverged=0; flaky=0; timedout=0

# Is a mode reproducible? Run it twice more and see whether it says the same
# thing each time. Asked only when a divergence has already been seen.
self_consistent() {
    local case_file="$1" mode="$2"
    execute "$case_file" "$mode" "$mode.c"
    if ! same "$mode.a" "$mode.c" >/dev/null; then return 1; fi
    execute "$case_file" "$mode" "$mode.d"
    same "$mode.a" "$mode.d" >/dev/null
}

for case_file in "$CASES"/*.ws; do
    name="$(basename "$case_file" .ws)"
    if [ -n "$ONLY" ] && [[ "$name" != *$ONLY* ]]; then continue; fi
    total=$((total + 1))
    case_started="$(date +%s)"

    for mode in run stress build; do
        execute "$case_file" "$mode" "$mode.a"
    done

    # Did any mode fail to finish inside the bound? `timeout` answers 124 (TERM)
    # or 137 (KILL), and both are the harness's number rather than the
    # program's: a mode that hit the bound has not disagreed with anything, it
    # has not answered. Decided before the comparison, and before any
    # self-consistency re-runs -- those would spend the bound twice more to
    # answer a determinism question a timeout is not.
    timeouts=""; answered=()
    for mode in run stress build; do
        if is_timeout "$mode.a"; then
            # The bound that mode actually ran under: the case's own, or the
            # compiler's when the build timed out before it had a program.
            timeouts="$timeouts $mode($(cat "$WORK/$mode.a.bound"))"
        else
            answered+=("$mode")
        fi
    done
    if [ -n "$timeouts" ]; then
        record_timeout "$name" "$timeouts"
    fi

    # The modes that answered are still compared. A timeout in one says nothing
    # about whether the other two agree, and skipping them would let a real
    # divergence hide behind a slow mode. `run` is the reference when it
    # answered, being the mode everything else is compared to in ordinary use.
    detail=""; unstable=""
    ref="${answered[0]:-}"
    for other in "${answered[@]:1}"; do
        what="$(same "$ref.a" "$other.a")" && continue
        # A difference. Before calling it a divergence, ask whether either side
        # even agrees with itself -- a case that prints a port number or a
        # temporary path does not, and that is a different finding with a
        # different owner.
        if ! self_consistent "$case_file" "$ref"; then
            unstable="$unstable $ref"
            record_diff "$name" "nondeterministic-$ref" "$ref.a" "$ref.c"
            break
        fi
        if ! self_consistent "$case_file" "$other"; then
            unstable="$unstable $other"
            record_diff "$name" "nondeterministic-$other" "$other.a" "$other.c"
            continue
        fi
        detail="$detail $ref-vs-$other($what)"
        record_diff "$name" "$ref-vs-$other" "$ref.a" "$other.a"
    done

    case_elapsed=$(( $(date +%s) - case_started ))
    # A divergence between the modes that answered outranks a timeout in one
    # that did not: it is the P1. The timeout is still named in the row.
    if [ -n "$detail" ]; then
        diverged=$((diverged + 1))
        printf '%s\tDIVERGED\t%s\t%s\n' "$name" "$case_elapsed" \
            "${detail# }${timeouts:+; did not finish:$timeouts}" >>"$TSV"
    elif [ -n "$timeouts" ]; then
        timedout=$((timedout + 1))
        printf '%s\tTIMEOUT\t%s\t%s\n' "$name" "$case_elapsed" "did not finish:${timeouts}" >>"$TSV"
    elif [ -n "$unstable" ]; then
        flaky=$((flaky + 1))
        printf '%s\tNONDETERMINISTIC\t%s\t%s\n' "$name" "$case_elapsed" "${unstable# }" >>"$TSV"
    else
        agreed=$((agreed + 1))
        printf '%s\tAGREED\t%s\t\n' "$name" "$case_elapsed" >>"$TSV"
    fi
done

elapsed=$(( $(date +%s) - started ))

{
    echo "# JIT/AOT parity — $STAMP"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| commit at the start of the run | \`$SHA_AT_START\` |"
    echo "| commit now | \`$(source_sha)\` |"
    echo "| compiler source last changed | \`$COMPILER_SHA_AT_START\` |"
    echo "| platform | $(platform_line) |"
    echo "| compiler | \`$WSHARP\` |"
    echo "| modes | \`run\`, \`run --gc-stress\`, \`build\`+exec |"
    echo "| cases | $total |"
    echo "| per-run timeout | ${TIMEOUT}s default (build ${BUILD_TIMEOUT}s); a case may set its own with \`// timeout:\` |"
    echo "| duration | ${elapsed}s |"
    echo "| cores | $(nproc 2>/dev/null || echo unknown) |"
    echo "| load average at the end | $(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null || echo unknown) |"
    echo
    echo "## Result"
    echo
    echo "| verdict | count |"
    echo "|---|---|"
    echo "| agreed in all three modes | $agreed |"
    echo "| diverged between modes | $diverged |"
    echo "| nondeterministic within one mode | $flaky |"
    echo "| timed out inside the per-run bound | $timedout |"
    echo
    if [ "$timedout" -gt 0 ]; then
        echo "## Timed out"
        echo
        echo "A mode that did not finish inside its bound -- ${TIMEOUT}s unless the case"
        echo "names its own with \`// timeout:\`, ${BUILD_TIMEOUT}s to compile -- as each row says. This is"
        echo "**not** a divergence: the mode did not answer, so there is nothing"
        echo "to disagree with. Separate the two before routing -- a case the"
        echo "bound is too small for is corpus cost, and a mode that never"
        echo "finishes at a generous bound is a hang (criterion 5 clause 3)."
        echo
        awk -F'\t' '$2=="TIMEOUT" {printf("- `%s` — %s\n", $1, $4)}' "$TSV"
        echo
    fi
    if [ "$diverged" -gt 0 ]; then
        echo "## Divergences"
        echo
        awk -F'\t' '$2=="DIVERGED" {printf("- `%s` — %s\n", $1, $4)}' "$TSV"
        echo
    fi
    if [ "$flaky" -gt 0 ]; then
        echo "## Nondeterministic (excluded from the parity comparison)"
        echo
        echo "A case that does not agree with itself across two runs of one mode."
        echo "Classify before routing: timing, ordering, resource exhaustion, or"
        echo "uninitialised state each have a different owner."
        echo
        awk -F'\t' '$2=="NONDETERMINISTIC" {printf("- `%s` — disagrees with itself in: %s\n", $1, $4)}' "$TSV"
        echo
    fi
    echo "## The ten slowest cases"
    echo
    echo "Kept because the corpus's cost is the nightly matrix's cost, and a"
    echo "case that grows a minute is worth noticing before it is ten."
    echo
    echo '| case | seconds (three modes) |'
    echo '|---|---|'
    awk -F'\t' 'NR>1 {printf("%s\t%s\n", $3, $1)}' "$TSV" | sort -rn | head -10 \
        | awk -F'\t' '{printf("| `%s` | %s |\n", $2, $1)}'
    echo
    echo "Outputs for every case above are in \`diffs/\`."
} >"$OUT/report.md"

echo "parity: $agreed agreed, $diverged diverged, $flaky nondeterministic, $timedout timed out, of $total in ${elapsed}s"
echo "parity: $OUT/report.md"
[ "$diverged" -eq 0 ] && [ "$timedout" -eq 0 ]
