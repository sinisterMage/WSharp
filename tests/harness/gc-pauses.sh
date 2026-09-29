#!/usr/bin/env bash
# Collector pause times, and the counts that say whether they mean anything.
#
# **Every pause, not one number per run.** Criterion 8.2 asks for p50, p90, p99
# and max over at least 1,000 *pauses*. Until #24 the runtime kept a count, a
# total and a maximum per worker and nothing else, so a percentile over the
# pauses inside a run could not be computed at all. It now keeps every pause on
# request: `WSHARP_GC_PAUSE_LOG=<path>` writes one line per pause -- the
# microseconds, which of the three pauses, which worker -- and a trailer per
# worker saying how many it kept of how many it saw. This script sets it for
# every run and pools the lines into `pauses.tsv`, which is the raw data the
# distribution is recomputable from (Form B).
#
# The per-run table is kept beside it, because it answers a different question
# -- how bad is the worst pause in one program -- and because a run whose log
# went missing still has its statistics line.
#
# **The counts are checked, not assumed.** A stack walk that finds no roots
# makes every root check pass for the wrong reason, and a run with no trace and
# no pause would report a beautiful zero. Any case whose run reports zero
# collections, zero roots or zero pauses is called out, and so is a log that
# says it truncated, and a pooled population short of the criterion's 1,000.
#
# Usage:
#   tests/harness/gc-pauses.sh [--repeats N] [--only PATTERN] [--stress]

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

REPEATS=5
ONLY="gc_"
STRESS=0

while [ $# -gt 0 ]; do
    case "$1" in
        --repeats) REPEATS="$2"; shift 2 ;;
        --only) ONLY="$2"; shift 2 ;;
        --stress) STRESS=1; shift ;;
        -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "gc-pauses: unknown argument $1" >&2; exit 2 ;;
    esac
done

require_compiler

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${WSHARP_HARNESS_REPORTS:-$REPORTS/gc-pauses-$STAMP}"
mkdir -p "$OUT"
SAMPLES="$OUT/samples.tsv"
printf 'case\trepeat\tcollections\troots\ttraces\tmoved\tpauses\tmax_us\ttotal_us\tmean_us\twall_ms\n' >"$SAMPLES"
PAUSES="$OUT/pauses.tsv"
printf 'case\trepeat\tus\tpause\tworker\n' >"$PAUSES"
TRUNCATED="$OUT/truncated.txt"
: >"$TRUNCATED"

# Pull the numbers out of the statistics line. Parsed by name rather than by
# position, so a new counter inserted in the middle does not silently shift
# every column.
#
#   W# gc: 8 collections, 8 roots seen, 5000 freed, 100 live of 5200 allocated
#   (4800 live bytes of 249600), 2 traces, 100 moved,
#   5 pauses (0 served parked, max 246 us, total 750 us), ...
parse_stats() {
    awk '
        /^W# gc: / {
            line = $0
            collections = extract(line, "([0-9]+) collections")
            roots       = extract(line, "([0-9]+) roots seen")
            traces      = extract(line, "([0-9]+) traces")
            moved       = extract(line, "([0-9]+) moved")
            pauses      = extract(line, "([0-9]+) pauses")
            maxus       = extract(line, "max ([0-9]+) us")
            totalus     = extract(line, "total ([0-9]+) us")
            printf("%s\t%s\t%s\t%s\t%s\t%s\t%s\n",
                   collections, roots, traces, moved, pauses, maxus, totalus)
            found = 1
        }
        END { if (!found) printf("-\t-\t-\t-\t-\t-\t-\n") }
        function extract(s, pat,    m) {
            # mawk has no match groups; find the number next to the label.
            if (match(s, pat) == 0) return "-"
            m = substr(s, RSTART, RLENGTH)
            sub(/[^0-9]*/, "", m)
            sub(/[^0-9].*$/, "", m)
            return m
        }
    '
}

flags=()
MODE_LABEL="wsharp run"
if [ "$STRESS" -eq 1 ]; then
    flags+=(--gc-stress)
    MODE_LABEL="wsharp run --gc-stress"
fi

# Pinned once, at the start. The working tree can move under a long run -- this
# is a shared checkout -- and a report naming the SHA it finished at rather than
# the one it measured is a report that lies about what was measured.
SHA_AT_START="$(source_sha)"

for case_file in "$CASES"/*.ws; do
    name="$(basename "$case_file" .ws)"
    [[ "$name" == *$ONLY* ]] || continue
    args="$(case_args "$case_file")"
    for r in $(seq 1 "$REPEATS"); do
        log="$OUT/pause-$name-$r.log"
        start_ns="$(date +%s%N)"
        # shellcheck disable=SC2086
        stats="$(WSHARP_GC_STATS=1 WSHARP_GC_PAUSE_LOG="$log" "$WSHARP" run "${flags[@]}" "$case_file" $args \
                 2>&1 >/dev/null | parse_stats)"
        end_ns="$(date +%s%N)"
        wall=$(( (end_ns - start_ns) / 1000000 ))
        pauses="$(echo "$stats" | cut -f5)"
        total="$(echo "$stats" | cut -f7)"
        mean="-"
        if [ "$pauses" != "-" ] && [ "$pauses" -gt 0 ] 2>/dev/null; then
            mean=$(( total / pauses ))
        fi
        printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$r" "$stats" "$mean" "$wall" >>"$SAMPLES"
        if [ -f "$log" ]; then
            awk -F'\t' -v c="$name" -v r="$r" '!/^#/ && NF == 3 { print c "\t" r "\t" $0 }' "$log" >>"$PAUSES"
            grep -H "TRUNCATED" "$log" >>"$TRUNCATED" || true
            rm -f "$log"
        fi
    done
done

# Percentiles over a column, nearest-rank: the p-th percentile is the value at
# ceil(p/100 * n) of the sorted samples. Stated because there are several
# definitions and a number without one cannot be compared to a later number.
pct() {
    local col="$1" p="$2"
    awk -F'\t' -v c="$col" -v p="$p" '
        NR > 1 && $c != "-" { v[++n] = $c + 0 }
        END {
            if (n == 0) { print "-"; exit }
            for (i = 1; i <= n; i++)
                for (j = i + 1; j <= n; j++)
                    if (v[j] < v[i]) { t = v[i]; v[i] = v[j]; v[j] = t }
            k = int(p / 100 * n + 0.999999); if (k < 1) k = 1; if (k > n) k = n
            print v[k]
        }' "$SAMPLES"
}

# The same, over the pooled pauses, optionally of one kind only.
pause_pct() {
    local p="$1" kind="${2:-}"
    awk -F'\t' -v k="$kind" 'NR > 1 && (k == "" || $4 == k) { print $3 + 0 }' "$PAUSES" |
        sort -n | awk -v p="$p" '
            { v[++n] = $1 }
            END {
                if (n == 0) { print "-"; exit }
                k = int(p / 100 * n + 0.999999); if (k < 1) k = 1; if (k > n) k = n
                print v[k]
            }'
}
pause_count() {
    awk -F'\t' -v k="${1:-}" 'NR > 1 && (k == "" || $4 == k) { n++ } END { print n + 0 }' "$PAUSES"
}

sum_col() { awk -F'\t' -v c="$1" 'NR>1 && $c!="-" {s+=$c} END {print s+0}' "$SAMPLES"; }
count_rows() { awk -F'\t' 'NR>1 {n++} END {print n+0}' "$SAMPLES"; }

{
    echo "# Collector pause times — $STAMP"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| commit at the start of the run | \`$SHA_AT_START\` |"
    echo "| commit now | \`$(source_sha)\` |"
    echo "| platform | $(platform_line) |"
    echo "| mode | \`$MODE_LABEL\` |"
    echo "| cases | \`*${ONLY}*\` |"
    echo "| repeats per case | $REPEATS |"
    echo "| samples | $(count_rows) |"
    echo "| load average at the end | $(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null || echo unknown) |"
    echo "| cores | $(nproc 2>/dev/null || echo unknown) |"
    echo
    echo "## Method"
    echo
    echo "Every pause of every run, from \`WSHARP_GC_PAUSE_LOG\`, pooled into"
    echo "\`pauses.tsv\`: one row per pause, so the distribution below is"
    echo "**per-pause**. Beside it, one sample per run from \`WSHARP_GC_STATS=1\`"
    echo "(\`samples.tsv\`): that run's longest pause, and its total over its count."
    echo "Percentiles are nearest-rank -- the value at ceil(p/100 * n) of the sorted"
    echo "samples."
    echo
    echo "A pause is wall-clock time on the mutator thread, so on a loaded or"
    echo "single-core machine it includes time the thread was not scheduled."
    echo "Read the load average and core count above before treating a tail"
    echo "number as the collector's: a long tail on a busy box is a number to"
    echo "reproduce on a quiet one, not a defect."
    echo
    echo "## Per-pause distribution (microseconds)"
    echo
    total_pauses="$(pause_count)"
    echo "| pauses | n | p50 | p90 | p99 | max |"
    echo "|---|---|---|---|---|---|"
    echo "| all | $total_pauses | $(pause_pct 50) | $(pause_pct 90) | $(pause_pct 99) | $(pause_pct 100) |"
    for kind in initial mark-done evac-done; do
        echo "| \`$kind\` | $(pause_count "$kind") | $(pause_pct 50 "$kind") | $(pause_pct 90 "$kind") | $(pause_pct 99 "$kind") | $(pause_pct 100 "$kind") |"
    done
    echo
    if [ "$total_pauses" -lt 1000 ]; then
        echo "**Short of criterion 8.2:** $total_pauses pauses, and the criterion asks for"
        echo "at least 1,000. Raise \`--repeats\`."
    else
        echo "$total_pauses pauses: criterion 8.2's floor of 1,000 is met."
    fi
    if [ -s "$TRUNCATED" ]; then
        echo
        echo "**A log truncated,** so the population is missing pauses:"
        echo
        sed 's/^/- /' "$TRUNCATED"
    fi
    echo
    echo "## Per-run summary (microseconds)"
    echo
    echo "| statistic | p50 | p90 | p99 | max observed |"
    echo "|---|---|---|---|---|"
    echo "| longest pause per run | $(pct 8 50) | $(pct 8 90) | $(pct 8 99) | $(pct 8 100) |"
    echo "| mean pause per run | $(pct 10 50) | $(pct 10 90) | $(pct 10 99) | $(pct 10 100) |"
    echo "| pauses per run | $(pct 7 50) | $(pct 7 90) | $(pct 7 99) | $(pct 7 100) |"
    echo "| wall clock per run (ms) | $(pct 11 50) | $(pct 11 90) | $(pct 11 99) | $(pct 11 100) |"
    echo
    echo "Totals across every sample: $(sum_col 3) collections, $(sum_col 4) roots seen,"
    echo "$(sum_col 5) traces, $(sum_col 6) objects moved, $(sum_col 7) pauses."
    echo
    echo "## Counts that must not be zero"
    echo
    echo "A root walk that finds nothing makes every root check pass vacuously,"
    echo "so a case reporting zero here is a finding about the harness or the"
    echo "collector, not a quiet success."
    echo
    zero="$(awk -F'\t' 'NR>1 && ($3=="-" || $3==0 || $4==0 || $7==0) {printf("- `%s` repeat %s: collections=%s roots=%s pauses=%s\n", $1,$2,$3,$4,$7)}' "$SAMPLES")"
    if [ -z "$zero" ]; then
        echo "Every sample reported a non-zero collection count, root count and pause count."
    else
        echo "$zero"
    fi
    echo
    echo "## Worst case by longest pause"
    echo
    echo '| case | repeat | max pause (us) | pauses | moved |'
    echo '|---|---|---|---|---|'
    awk -F'\t' 'NR>1 && $8!="-" {printf("%s\t%s\t%s\t%s\t%s\n", $8,$1,$2,$7,$6)}' "$SAMPLES" \
        | sort -rn | head -10 \
        | awk -F'\t' '{printf("| `%s` | %s | %s | %s | %s |\n", $2,$3,$1,$4,$5)}'
    echo
    echo "Raw data: \`pauses.tsv\` (one row per pause) and \`samples.tsv\` (one per run)."
} >"$OUT/report.md"

echo "gc-pauses: $(pause_count) pauses over $(count_rows) runs; per pause p50=$(pause_pct 50)us p90=$(pause_pct 90)us p99=$(pause_pct 99)us max=$(pause_pct 100)us"
echo "gc-pauses: $OUT/report.md"
