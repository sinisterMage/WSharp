#!/usr/bin/env bash
# Watch one long-running process and record the numbers that only move over days.
#
# The defects a soak finds are monotone trends, not single spikes: resident size
# that climbs a megabyte an hour, a descriptor count that never comes back down,
# a pause tail that grows as the heap ages, throughput that decays. None of those
# is visible in a test that runs for a minute, and all of them are visible in a
# table of samples if somebody takes the samples.
#
# So this takes samples. It does not judge them -- `compare.sh` does that against
# the previous window, because a trend needs two windows and a single run of this
# is one.
#
# Usage:
#   tests/harness/soak.sh --command "<what to run>" \
#                         [--duration 3600] [--interval 30] \
#                         [--probe "<command>"] [--out DIR] [--label NAME]
#
#   --command   the process to soak. Run with no shell: the words are the argv.
#   --duration  seconds to soak for. 0 means "until the process exits".
#   --interval  seconds between samples.
#   --probe     a command run at every sample, timed in milliseconds. This is
#               the latency number; for a server it is a request, and for
#               something with no request surface it is omitted.
#   --label     what to call this window in the report.
#
# What a sample holds: elapsed seconds, resident size in KiB, open descriptor
# count, cumulative stdout lines (throughput), and the probe's duration and exit
# status. Written to `counters.tsv` *as it goes*, so a window that is cut short
# -- by a machine going away, which over a week happens -- still leaves every
# sample it took rather than nothing.
#
# The collector's own numbers are read at the end, from `WSHARP_GC_STATS=1` on
# stderr, which the runtime prints on exit. Nothing can ask a running W# process
# for them today; that is the same observability gap `gc-pauses.sh` names.

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

COMMAND=""
PROBE=""
DURATION=3600
INTERVAL=30
LABEL="soak"
OUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --command) COMMAND="$2"; shift 2 ;;
        --probe) PROBE="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --interval) INTERVAL="$2"; shift 2 ;;
        --label) LABEL="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,36p' "$0"; exit 0 ;;
        *) echo "soak: unknown argument $1" >&2; exit 2 ;;
    esac
done

if [ -z "$COMMAND" ]; then
    echo "soak: --command is required" >&2
    exit 2
fi

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${OUT:-$REPORTS/soak-$LABEL-$STAMP}"
mkdir -p "$OUT"
SHA_AT_START="$(source_sha)"

STDOUT_LOG="$OUT/process.stdout"
STDERR_LOG="$OUT/process.stderr"
COUNTERS="$OUT/counters.tsv"
printf 'iso\telapsed_s\trss_kib\tfds\tstdout_lines\tprobe_ms\tprobe_exit\tloadavg1\n' >"$COUNTERS"

# The collector prints its statistics on exit, so ask for them.
export WSHARP_GC_STATS=1

# shellcheck disable=SC2086
setsid $COMMAND >"$STDOUT_LOG" 2>"$STDERR_LOG" &
PID=$!
echo "$PID" >"$OUT/pid"
echo "soak: pid $PID; samples every ${INTERVAL}s for ${DURATION}s into $OUT"

cleanup() {
    if kill -0 "$PID" 2>/dev/null; then
        kill -TERM "-$PID" 2>/dev/null || kill -TERM "$PID" 2>/dev/null
        sleep 2
        kill -KILL "-$PID" 2>/dev/null || kill -KILL "$PID" 2>/dev/null
    fi
}
trap cleanup EXIT INT TERM

started="$(date +%s)"
samples=0
exited_early=0

while :; do
    now="$(date +%s)"
    elapsed=$(( now - started ))
    if [ "$DURATION" -gt 0 ] && [ "$elapsed" -ge "$DURATION" ]; then break; fi
    if ! kill -0 "$PID" 2>/dev/null; then exited_early=1; break; fi

    rss="-"
    if [ -r "/proc/$PID/status" ]; then
        rss="$(awk '/^VmRSS:/ {print $2}' "/proc/$PID/status" 2>/dev/null)"
        rss="${rss:--}"
    fi
    fds="-"
    if [ -d "/proc/$PID/fd" ]; then
        # `ls` rather than a glob: a process can hold more descriptors than a
        # command line can take arguments for.
        fds="$(ls "/proc/$PID/fd" 2>/dev/null | wc -l)"
    fi
    lines="$(wc -l <"$STDOUT_LOG" 2>/dev/null | tr -d ' ')"
    load="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo -)"

    probe_ms="-"; probe_exit="-"
    if [ -n "$PROBE" ]; then
        p0="$(date +%s%N)"
        # shellcheck disable=SC2086
        $PROBE >/dev/null 2>&1
        probe_exit=$?
        p1="$(date +%s%N)"
        probe_ms=$(( (p1 - p0) / 1000000 ))
    fi

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$elapsed" "$rss" "$fds" \
        "${lines:-0}" "$probe_ms" "$probe_exit" "$load" >>"$COUNTERS"
    samples=$((samples + 1))
    sleep "$INTERVAL"
done

ran=$(( $(date +%s) - started ))
cleanup
trap - EXIT INT TERM
wait "$PID" 2>/dev/null
process_exit=$?

# Percentiles over a column of counters.tsv, nearest-rank.
pct() {
    awk -F'\t' -v c="$1" -v p="$2" '
        NR>1 && $c != "-" { v[++n] = $c + 0 }
        END {
            if (n == 0) { print "-"; exit }
            for (i=1;i<=n;i++) for (j=i+1;j<=n;j++) if (v[j]<v[i]) {t=v[i];v[i]=v[j];v[j]=t}
            k = int(p/100*n + 0.999999); if (k<1) k=1; if (k>n) k=n
            print v[k]
        }' "$COUNTERS"
}
first() { awk -F'\t' -v c="$1" 'NR==2 {print $c; exit}' "$COUNTERS"; }
last()  { awk -F'\t' -v c="$1" '{v=$c} END {print v}' "$COUNTERS"; }

# A straight-line fit over (elapsed, value), in units per hour.
#
# Taken over the **second half** of the window by default, and that is not a
# refinement. A process that reaches its working set in the first few seconds --
# which every allocator does, and the self-test in `drivers/soak_self.ws` does
# in eleven -- has a first sample far below every later one, and a fit over the
# whole window reads that step as a rate. The self-test's RSS goes 12 MiB to
# 87 MiB in eleven seconds and then moves 600 KiB in the next eighty; fitted
# over everything that is "+1.6 GiB per hour", which would be a leak if it were
# true, and over the second half it is nearly flat, which is what is happening.
#
# Warm-up is a step and creep is a slope. Passing `all` gives the whole-window
# fit for comparison, and both are printed.
slope_per_hour() {
    local col="$1" span="${2:-half}"
    awk -F'\t' -v c="$col" -v span="$span" '
        NR>1 && $c != "-" { t[++m] = $2 + 0; v[m] = $c + 0; last = $2 + 0 }
        END {
            if (m < 3) { print "too few samples"; exit }
            from = (span == "half") ? last / 2 : -1
            for (i = 1; i <= m; i++) {
                if (t[i] < from) continue
                n++; x = t[i]; y = v[i]
                sx += x; sy += y; sxx += x*x; sxy += x*y
            }
            if (n < 3) { print "too few samples in span"; exit }
            d = n*sxx - sx*sx
            if (d == 0) { print "-"; exit }
            printf("%+.1f", (n*sxy - sx*sy) / d * 3600)
        }' "$COUNTERS"
}

{
    echo "# Soak — $LABEL — $STAMP"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| commit at the start | \`$SHA_AT_START\` |"
    echo "| platform | $(platform_line) |"
    echo "| cores | $(nproc 2>/dev/null || echo unknown) |"
    echo "| command | \`$COMMAND\` |"
    echo "| probe | ${PROBE:+\`$PROBE\`}${PROBE:-none} |"
    echo "| asked for | ${DURATION}s |"
    echo "| actually ran | ${ran}s |"
    echo "| samples | $samples, every ${INTERVAL}s |"
    if [ "$exited_early" -eq 1 ]; then
        echo "| process | **exited before the window elapsed** (status $process_exit) |"
    else
        echo "| process | still running when the window ended; terminated |"
    fi
    echo
    if [ "$exited_early" -eq 1 ] && [ "$DURATION" -gt 0 ]; then
        echo "> The window did not elapse. Read what follows as ${ran}s of soak,"
        echo "> not ${DURATION}s, and do not call this window clean."
        echo
    fi
    echo "## Trend"
    echo
    echo "| counter | first | last | p50 | p99 | slope/h (2nd half) | slope/h (all) |"
    echo "|---|---|---|---|---|---|---|"
    echo "| resident size (KiB) | $(first 3) | $(last 3) | $(pct 3 50) | $(pct 3 99) | $(slope_per_hour 3) | $(slope_per_hour 3 all) |"
    echo "| open descriptors | $(first 4) | $(last 4) | $(pct 4 50) | $(pct 4 99) | $(slope_per_hour 4) | $(slope_per_hour 4 all) |"
    if [ -n "$PROBE" ]; then
        echo "| probe latency (ms) | $(first 6) | $(last 6) | $(pct 6 50) | $(pct 6 99) | $(slope_per_hour 6) | $(slope_per_hour 6 all) |"
    fi
    echo "| load average | $(first 8) | $(last 8) | $(pct 8 50) | $(pct 8 99) | — | — |"
    echo
    echo "Throughput: $(last 5) stdout lines over ${ran}s."
    echo
    echo "**Read the second-half slope.** The whole-window one includes warm-up,"
    echo "which is a step rather than a rate, and reads as an enormous leak on any"
    echo "process that reaches its working set quickly. A positive second-half"
    echo "slope in resident size or in descriptors is the finding; a single high"
    echo "sample is not, which is what the p50 beside the p99 is for."
    echo
    echo "A slope measured over a window this short says little either way."
    echo "The class of defect this is for shows up over days."
    echo
    echo "## What the collector did"
    echo
    echo '```'
    grep '^W# gc: ' "$STDERR_LOG" 2>/dev/null || echo "(no statistics line: the process did not exit normally)"
    echo '```'
    echo
    echo "## Compare against the previous window"
    echo
    echo '```sh'
    echo "tests/harness/compare.sh <previous soak dir> $OUT"
    echo '```'
    echo
    echo "Raw samples: \`counters.tsv\`. Process output: \`process.stdout\`,"
    echo "\`process.stderr\`."
} >"$OUT/report.md"

echo "soak: $samples samples over ${ran}s; RSS $(first 3) -> $(last 3) KiB, slope $(slope_per_hour 3) KiB/h"
echo "soak: $OUT/report.md"
