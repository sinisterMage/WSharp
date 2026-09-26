#!/usr/bin/env bash
# One night's adversarial run on one platform, and one readable report.
#
# This is the per-platform half. The matrix is five of these -- the four release
# platforms and the older-glibc target -- each writing a report into an artifact
# directory named for its platform; the workflow that fans them out and collects
# them is CI's, not this script's, because a script that knew it was in CI would
# be a second place the matrix is defined.
#
# What it runs, in this order, cheapest first so a break is reported early:
#
#   1. the corpus in all three execution modes, compared against each other
#      (`parity.sh`)
#   2. collector pause times and the counts that say they mean something
#      (`gc-pauses.sh`), with and without stress
#   3. a bounded fuzz campaign per target, with the night's date as the seed
#      (`fuzz.pl`) -- a fixed seed per night, so a finding names the night it
#      came from and can be replayed exactly
#
# Deliberately not here: `cargo test --workspace`. That is CI's existing job and
# it is a different question -- whether the suite passes. This asks whether
# anything disagrees with anything else, which a green suite does not answer.
#
# Usage:
#   tests/harness/nightly.sh [--out DIR] [--fuzz-seconds N] [--quick]

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

FUZZ_SECONDS=1800
QUICK=0
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
NIGHT="$(date -u +%Y%m%d)"
OUT="$REPORTS/nightly-$STAMP"

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="$2"; shift 2 ;;
        --fuzz-seconds) FUZZ_SECONDS="$2"; shift 2 ;;
        --quick) QUICK=1; shift ;;
        -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
        *) echo "nightly: unknown argument $1" >&2; exit 2 ;;
    esac
done

require_compiler
mkdir -p "$OUT"

SHA_AT_START="$(source_sha)"
PLATFORM="$(platform_line)"
started="$(date +%s)"

# The seed is the date. One number in the report is then enough to replay the
# whole night, and two nights never run the same inputs.
SEED="$NIGHT"

status_parity="not run"
status_pauses="not run"
declare -A status_fuzz

echo "nightly: parity over the corpus"
if WSHARP_HARNESS_REPORTS="$OUT/parity" bash "$(dirname "${BASH_SOURCE[0]}")/parity.sh" \
        --timeout 300 >"$OUT/parity.log" 2>&1; then
    status_parity="no divergence"
else
    status_parity="divergences — see parity/report.md"
fi

echo "nightly: collector pauses"
repeats=5
[ "$QUICK" -eq 1 ] && repeats=2
if WSHARP_HARNESS_REPORTS="$OUT/gc-pauses" bash "$(dirname "${BASH_SOURCE[0]}")/gc-pauses.sh" \
        --only gc_ --repeats "$repeats" >"$OUT/gc-pauses.log" 2>&1; then
    status_pauses="measured"
else
    status_pauses="failed — see gc-pauses.log"
fi
if [ "$QUICK" -eq 0 ]; then
    WSHARP_HARNESS_REPORTS="$OUT/gc-pauses-stress" bash "$(dirname "${BASH_SOURCE[0]}")/gc-pauses.sh" \
        --only gc_ --repeats 3 --stress >"$OUT/gc-pauses-stress.log" 2>&1 || true
fi

per_target=$(( FUZZ_SECONDS / 4 ))
for target in check run json toml; do
    echo "nightly: fuzzing $target for ${per_target}s"
    if "$(dirname "${BASH_SOURCE[0]}")/fuzz.pl" --target "$target" --seed "$SEED" \
            --iterations 100000 --max-seconds "$per_target" --timeout 25 \
            --out "$OUT/fuzz-$target" >>"$OUT/fuzz.log" 2>&1; then
        status_fuzz[$target]="no finding"
    else
        status_fuzz[$target]="findings — see fuzz-$target/report.md"
    fi
done

elapsed=$(( $(date +%s) - started ))

summarise_parity() {
    local tsv="$OUT/parity/results.tsv"
    [ -f "$tsv" ] || { echo "not run"; return; }
    awk -F'\t' 'NR>1 {n[$2]++} END {printf("%d agreed, %d diverged, %d nondeterministic", n["AGREED"], n["DIVERGED"], n["NONDETERMINISTIC"])}' "$tsv"
}

{
    echo "# W# nightly adversarial run — $STAMP"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| commit | \`$SHA_AT_START\` |"
    echo "| platform | $PLATFORM |"
    echo "| cores | $(nproc 2>/dev/null || echo unknown) |"
    echo "| seed | \`$SEED\` |"
    echo "| duration | ${elapsed}s |"
    echo
    echo "## Summary"
    echo
    echo "| stage | result |"
    echo "|---|---|"
    echo "| JIT/AOT/stress parity | $(summarise_parity) |"
    echo "| collector pauses | $status_pauses |"
    for target in check run json toml; do
        echo "| fuzz \`$target\` | ${status_fuzz[$target]} |"
    done
    echo
    if [ -f "$OUT/gc-pauses/report.md" ]; then
        echo "## Pause times"
        echo
        sed -n '/^## Pause distribution/,/^$/p;/^| longest pause/p;/^| mean pause/p' \
            "$OUT/gc-pauses/report.md" | head -20
        echo
    fi
    echo "## New failures since the previous night"
    echo
    echo "Compare with the previous night's directory:"
    echo
    echo '```sh'
    echo "tests/harness/compare.sh <previous nightly dir> $OUT"
    echo '```'
    echo
    echo "## Where everything is"
    echo
    echo "| file | what |"
    echo "|---|---|"
    echo "| \`parity/report.md\` | per-case verdicts, divergences, the slowest cases |"
    echo "| \`parity/results.tsv\` | one row per case, for comparing two nights |"
    echo "| \`parity/diffs/\` | the actual outputs of every case that disagreed |"
    echo "| \`gc-pauses/report.md\` | pause percentiles and the non-zero count check |"
    echo "| \`gc-pauses/samples.tsv\` | one row per run, for a week-over-week trend |"
    echo "| \`fuzz-*/report.md\` | outcome counts and one section per distinct cause |"
    echo "| \`fuzz-*/findings/\` | a reduced input per cause, replayable with \`--replay\` |"
} >"$OUT/report.md"

echo "nightly: $OUT/report.md"
