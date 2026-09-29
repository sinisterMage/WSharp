#!/usr/bin/env bash
# Criterion 8.3: a benchmark baseline per release triple, and the 2x check
# against it.
#
# The severity scale makes "a performance regression of more than 2x on any
# case in `tests/cases`" a P2, which is a comparison -- and a comparison needs
# something committed to compare against. This writes that thing, and reads it
# back.
#
#   tests/harness/bench.sh --record              write benchmarks/baselines/<triple>.tsv
#   tests/harness/bench.sh --compare [FILE]      re-run, and hold every case to 2x of FILE
#
# **What is timed.** Every case that compiles, two ways:
#
#   run_ms   the built program, `wsharp build` then execute -- what a user ships
#   jit_ms   `wsharp run`, compilation included -- what a user waits for
#
# each the median of `--repeats` samples, all of which are kept in the file, so
# a percentile or a mean can be recomputed from it (Form B: raw data, committed,
# not summarised). A case that fails, or exceeds its bound, is recorded as `-`
# rather than dropped, so the file says which cases it could not time.
#
# **What counts as a regression.** More than twice the baseline *and* more than
# `--floor` milliseconds slower (default 50). The floor is what keeps a 2 ms case
# taking 5 ms -- which is timer and scheduler noise, not the compiler -- from
# being reported as a P2; a regression on a case that small is one somebody has
# to show on a larger input anyway.
#
# **The compiler should be a release build.** Timing a debug build measures the
# debug runtime. The file records which one was used, and `--compare` refuses a
# baseline recorded with a different profile than the compiler it is handed.
#
# Exit status: 0 recorded, or compared with nothing over 2x; 1 a regression, each
# named on stdout; 2 the question could not be answered.

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

MODE=""
REPEATS=5
FLOOR=50
FILE=""
TRIPLE=""
ONLY=""

while [ $# -gt 0 ]; do
    case "$1" in
        --record) MODE=record; shift ;;
        --compare)
            MODE=compare; shift
            if [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; then FILE="$1"; shift; fi
            ;;
        --out) FILE="$2"; shift 2 ;;
        --repeats) REPEATS="$2"; shift 2 ;;
        --floor) FLOOR="$2"; shift 2 ;;
        --triple) TRIPLE="$2"; shift 2 ;;
        --only) ONLY="$2"; shift 2 ;;
        -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
        *) echo "bench: unknown argument $1" >&2; exit 2 ;;
    esac
done
[ -n "$MODE" ] || { echo "bench: say --record or --compare" >&2; exit 2; }

require_compiler

# The release triple this machine is, spelled as the release names it.
if [ -z "$TRIPLE" ]; then
    case "$(uname -s)/$(uname -m)" in
        Linux/x86_64) TRIPLE=x86_64-unknown-linux-gnu ;;
        Darwin/arm64) TRIPLE=aarch64-apple-darwin ;;
        Darwin/x86_64) TRIPLE=x86_64-apple-darwin ;;
        MINGW*/x86_64 | MSYS*/x86_64 | CYGWIN*/x86_64) TRIPLE=x86_64-pc-windows-msvc ;;
        *) echo "bench: not a release triple: $(uname -s)/$(uname -m); pass --triple" >&2; exit 2 ;;
    esac
fi
FILE="${FILE:-$ROOT/benchmarks/baselines/$TRIPLE.tsv}"

# Which cargo profile built the compiler, from where it sits.
case "$WSHARP" in
    */target/release/*) PROFILE=release ;;
    */target/debug/*) PROFILE=debug ;;
    *) PROFILE=unknown ;;
esac

if [ "$MODE" = compare ]; then
    [ -f "$FILE" ] || { echo "bench: no baseline at $FILE -- record one with --record" >&2; exit 2; }
    base_profile="$(sed -n 's/^# profile[[:space:]]*//p' "$FILE" | head -1)"
    if [ "$base_profile" != "$PROFILE" ]; then
        echo "bench: $FILE was recorded with a $base_profile compiler and this one is $PROFILE;" >&2
        echo "bench: the two are not comparable" >&2
        exit 2
    fi
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-bench-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
EXE_SUFFIX=""
case "$TRIPLE" in *windows*) EXE_SUFFIX=".exe" ;; esac

now_ms() {
    # GNU date has %N; BSD date prints it literally, so fall back to perl there.
    local ns; ns="$(date +%s%N)"
    case "$ns" in
        *N) perl -MTime::HiRes=time -e 'printf("%d\n", time() * 1000)' ;;
        *) echo $(( ns / 1000000 )) ;;
    esac
}

# time_ms BOUND CMD... -- the wall-clock milliseconds CMD took, or `-` if it ran
# out of bound or could not be started. Any other exit counts: a `// panic:` or
# `// exit: 3` case ends that way on purpose, and its time is as real as any.
# Output is discarded: this is about time.
time_ms() {
    local bound="$1" start end code
    shift
    start="$(now_ms)"
    "$TIMEOUT_BIN" "$bound" "$@" >/dev/null 2>&1
    code=$?
    end="$(now_ms)"
    case "$code" in
        124 | 126 | 127) echo - ;;
        *) echo $(( end - start )) ;;
    esac
}

median() {
    tr ',' '\n' | grep -v '^-$' | sort -n |
        awk '{ v[++n] = $1 } END { if (n == 0) { print "-" } else { print v[int((n + 1) / 2)] } }'
}

# Pinned at the start: a run takes the better part of an hour, the working tree
# can move under it, and a baseline naming the commit it *finished* at names
# source the compiler was not built from.
COMMIT_AT_START="$(source_sha)"
COMPILER_AT_START="$(compiler_source_sha)"
LOAD_START="$(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null || uptime | sed 's/.*averages*: //')"
RESULTS="$WORK/results.tsv"
: >"$RESULTS"

for case_file in "$CASES"/*.ws; do
    name="$(basename "$case_file" .ws)"
    [ -z "$ONLY" ] || [[ "$name" == *$ONLY* ]] || continue
    case_expects_error "$case_file" && continue
    args="$(case_args "$case_file")"
    bound="$(case_timeout "$case_file")"; bound="${bound:-120}"

    exe="$WORK/$name$EXE_SUFFIX"
    run_samples="" jit_samples=""
    if "$TIMEOUT_BIN" 300 "$WSHARP" build "$case_file" -o "$exe" >/dev/null 2>&1; then
        for _ in $(seq 1 "$REPEATS"); do
            # shellcheck disable=SC2086
            run_samples="$run_samples,$(time_ms "$bound" "$exe" $args)"
        done
    fi
    for _ in $(seq 1 "$REPEATS"); do
        # shellcheck disable=SC2086
        jit_samples="$jit_samples,$(time_ms "$bound" "$WSHARP" run "$case_file" $args)"
    done
    rm -f "$exe"
    run_samples="${run_samples#,}"; jit_samples="${jit_samples#,}"
    printf '%s\t%s\t%s\t%s\t%s\n' "$name" \
        "$(printf '%s' "${run_samples:--}" | median)" "$(printf '%s' "$jit_samples" | median)" \
        "${run_samples:--}" "$jit_samples" >>"$RESULTS"
done
LOAD_END="$(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null || uptime | sed 's/.*averages*: //')"

if [ "$MODE" = record ]; then
    mkdir -p "$(dirname "$FILE")"
    {
        echo "# W# benchmark baseline, criterion 8.3 of RELEASE-CRITERIA-1.0.md"
        echo "# harness   tests/harness/bench.sh --record --repeats $REPEATS"
        echo "# commit    $COMMIT_AT_START"
        echo "# compiler  built from $COMPILER_AT_START"
        echo "# profile   $PROFILE"
        echo "# triple    $TRIPLE"
        echo "# platform  $(platform_line | tr -d '\n'); $(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo '?') cores"
        echo "# load      $LOAD_START at the start, $LOAD_END at the end"
        echo "# mode      run_ms: wsharp build, then the program; jit_ms: wsharp run, compile included"
        echo "# samples   $REPEATS per case per mode, all kept; the ms columns are their medians"
        echo "# re-run    tests/harness/bench.sh --record --repeats $REPEATS"
        printf 'case\trun_ms\tjit_ms\trun_samples\tjit_samples\n'
        cat "$RESULTS"
    } >"$FILE"
    echo "bench: $(wc -l <"$RESULTS" | tr -d ' ') cases, baseline written to $FILE"
    exit 0
fi

# --- compare -----------------------------------------------------------------

regressions=0
while IFS=$'\t' read -r name run jit _ _; do
    base="$(awk -F'\t' -v n="$name" '$1 == n { print $2 "\t" $3 }' "$FILE")"
    if [ -z "$base" ]; then
        echo "new   $name (not in the baseline)"
        continue
    fi
    base_run="${base%%$'\t'*}"; base_jit="${base#*$'\t'}"
    for pair in "run_ms:$base_run:$run" "jit_ms:$base_jit:$jit"; do
        IFS=: read -r what was now <<<"$pair"
        [ "$was" != "-" ] && [ "$now" != "-" ] || continue
        if [ "$now" -gt $(( was * 2 )) ] && [ $(( now - was )) -gt "$FLOOR" ]; then
            echo "SLOW  $name $what: ${was} ms -> ${now} ms"
            regressions=$((regressions + 1))
        fi
    done
    if [ "$run" = "-" ] && [ "$base_run" != "-" ]; then
        echo "LOST  $name run_ms: timed at ${base_run} ms in the baseline, cannot be timed now"
        regressions=$((regressions + 1))
    fi
done <"$RESULTS"

echo "bench: compared $(wc -l <"$RESULTS" | tr -d ' ') cases against $FILE (load $LOAD_START -> $LOAD_END)"
if [ "$regressions" -gt 0 ]; then
    echo "bench: $regressions case(s) more than 2x (and ${FLOOR} ms) slower than the baseline: a P2 each"
    exit 1
fi
echo "bench: nothing is more than 2x slower than the baseline"
