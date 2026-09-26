#!/usr/bin/env bash
# Run one case many times and report how often it does something different.
#
# A flake is a finding about determinism, and a finding needs a number. "It
# failed once" is not actionable and neither is "it is flaky"; what an owner can
# act on is "1 in 40, and when it fails it takes five times as long", because
# that distinguishes a stall from a machine that was busy.
#
# It also distinguishes the two things `parity.sh` cannot tell apart on its own.
# A case that exceeds a timeout under load may be slow or may be stuck, and the
# difference is whether the *duration distribution* has a tail or a second mode.
# So every attempt's duration is recorded, not just its verdict.
#
# Usage:
#   tests/harness/flake-rate.sh --case tests/cases/https_loopback.ws \
#       [--attempts 40] [--timeout 400] [--mode run|stress|build]

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

CASE=""
ATTEMPTS=40
TIMEOUT=400
MODE=run
OUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --case) CASE="$2"; shift 2 ;;
        --attempts) ATTEMPTS="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --mode) MODE="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
        *) echo "flake-rate: unknown argument $1" >&2; exit 2 ;;
    esac
done

[ -n "$CASE" ] || { echo "flake-rate: --case is required" >&2; exit 2; }
require_compiler

NAME="$(basename "$CASE" .ws)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${OUT:-$REPORTS/flake-$NAME-$STAMP}"
mkdir -p "$OUT"
SHA_AT_START="$(source_sha)"
TSV="$OUT/attempts.tsv"
printf 'attempt\texit\tseconds\tstdout_lines\tstderr_bytes\tloadavg1\n' >"$TSV"

args="$(case_args "$CASE")"
exe=""
if [ "$MODE" = build ]; then
    exe="$OUT/$NAME"
    "$WSHARP" build "$CASE" -o "$exe" >"$OUT/build.log" 2>&1 || {
        echo "flake-rate: build failed; see $OUT/build.log" >&2; exit 1; }
fi

for i in $(seq 1 "$ATTEMPTS"); do
    load="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo -)"
    s="$(date +%s)"
    case "$MODE" in
        # shellcheck disable=SC2086
        run)    timeout "$TIMEOUT" "$WSHARP" run "$CASE" $args >"$OUT/o" 2>"$OUT/e" ;;
        # shellcheck disable=SC2086
        stress) timeout "$TIMEOUT" "$WSHARP" run --gc-stress "$CASE" $args >"$OUT/o" 2>"$OUT/e" ;;
        # shellcheck disable=SC2086
        build)  timeout "$TIMEOUT" "$exe" $args >"$OUT/o" 2>"$OUT/e" ;;
    esac
    code=$?
    e="$(date +%s)"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$i" "$code" "$((e - s))" \
        "$(wc -l <"$OUT/o" | tr -d ' ')" "$(wc -c <"$OUT/e" | tr -d ' ')" "$load" >>"$TSV"
    # Keep the output of anything that did not exit 0, so a failure has evidence.
    if [ "$code" -ne 0 ]; then
        cp "$OUT/o" "$OUT/attempt-$i.stdout"
        cp "$OUT/e" "$OUT/attempt-$i.stderr"
    fi
done
rm -f "$OUT/o" "$OUT/e" "$exe"

pct() {
    awk -F'\t' -v c="$1" -v p="$2" '
        NR>1 { v[++n] = $c + 0 }
        END {
            if (n == 0) { print "-"; exit }
            for (i=1;i<=n;i++) for (j=i+1;j<=n;j++) if (v[j]<v[i]) {t=v[i];v[i]=v[j];v[j]=t}
            k = int(p/100*n + 0.999999); if (k<1) k=1; if (k>n) k=n
            print v[k]
        }' "$TSV"
}

{
    echo "# Flake rate — \`$NAME\` — $STAMP"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| commit at the start | \`$SHA_AT_START\` |"
    echo "| platform | $(platform_line) |"
    echo "| cores | $(nproc 2>/dev/null || echo unknown) |"
    echo "| mode | \`$MODE\` |"
    echo "| attempts | $ATTEMPTS |"
    echo "| per-attempt timeout | ${TIMEOUT}s |"
    echo
    echo "## Rate"
    echo
    awk -F'\t' 'NR>1 {n++; c[$2]++} END {
        print "| exit status | attempts | rate |"
        print "|---|---|---|"
        for (k in c) printf("| %s | %d | %d/%d |\n", k, c[k], c[k], n)
    }' "$TSV"
    echo
    echo "Exit 124 is the harness timeout, not the program."
    echo
    echo "## Duration"
    echo
    echo "| p50 | p90 | p99 | max |"
    echo "|---|---|---|---|"
    echo "| $(pct 3 50)s | $(pct 3 90)s | $(pct 3 99)s | $(pct 3 100)s |"
    echo
    echo "A long tail on a busy machine is slowness. A second mode far from the"
    echo "first -- an attempt that takes several times the p99 and then hits the"
    echo "timeout -- is a stall, and that is a defect whatever the load was."
    echo
    echo "## Attempts that did not exit 0"
    echo
    failed="$(awk -F'\t' 'NR>1 && $2!=0 {printf("- attempt %s: exit %s after %ss (load %s)\n", $1, $2, $3, $6)}' "$TSV")"
    if [ -z "$failed" ]; then
        echo "None in $ATTEMPTS attempts."
    else
        echo "$failed"
        echo
        echo "Their output is kept as \`attempt-N.stdout\` and \`attempt-N.stderr\`."
    fi
    echo
    echo "Raw attempts: \`attempts.tsv\`."
} >"$OUT/report.md"

echo "flake-rate: $NAME — $(awk -F'\t' 'NR>1 && $2!=0 {n++} END {print n+0}' "$TSV")/$ATTEMPTS did not exit 0; duration p50 $(pct 3 50)s p99 $(pct 3 99)s max $(pct 3 100)s"
echo "flake-rate: $OUT/report.md"
