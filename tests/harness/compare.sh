#!/usr/bin/env bash
# Compare two runs -- two nights, or two soak windows -- and say what moved.
#
# The point of a campaign is the *trend*. A single night's report says whether
# anything broke last night; two nights side by side say whether a case that
# used to agree has started to disagree, whether a pause tail is growing, and
# whether a fuzz cause is new or one already filed. Defects that matter over
# weeks are monotone trends rather than single spikes, and a spike is only
# legible against the line it departs from.
#
# Usage:
#   tests/harness/compare.sh <previous dir> <current dir>
#
# Either directory may be a `nightly-*` directory or a bare `parity-*` or
# `gc-pauses-*` one; whichever files are present are compared and the rest are
# reported as absent rather than silently skipped.

set -uo pipefail

PREV="${1:-}"
CURR="${2:-}"
if [ -z "$PREV" ] || [ -z "$CURR" ]; then
    sed -n '2,18p' "$0"
    exit 2
fi

# A file under either layout: `<dir>/parity/results.tsv` for a nightly, or
# `<dir>/results.tsv` for a bare parity run.
locate() {
    local dir="$1" sub="$2" leaf="$3"
    if [ -f "$dir/$sub/$leaf" ]; then echo "$dir/$sub/$leaf"
    elif [ -f "$dir/$leaf" ]; then echo "$dir/$leaf"
    fi
}

echo "# Comparison"
echo
echo "- previous: \`$PREV\`"
echo "- current:  \`$CURR\`"
echo

# ---------------------------------------------------------------------------
# Parity: what changed verdict
# ---------------------------------------------------------------------------
pa="$(locate "$PREV" parity results.tsv)"
pb="$(locate "$CURR" parity results.tsv)"
echo "## Parity"
echo
if [ -z "$pa" ] || [ -z "$pb" ]; then
    echo "Absent from one side: previous=\`${pa:-none}\` current=\`${pb:-none}\`."
else
    join -t"$(printf '\t')" -a1 -a2 -e MISSING -o 0,1.2,2.2 \
        <(tail -n +2 "$pa" | cut -f1,2 | sort) \
        <(tail -n +2 "$pb" | cut -f1,2 | sort) \
    | awk -F'\t' '
        $2 != $3 { changed[NR] = sprintf("| `%s` | %s | %s |", $1, $2, $3); n++ }
        END {
            if (n == 0) { print "Every case kept its verdict."; exit }
            print "| case | was | is |"
            print "|---|---|---|"
            for (i in changed) print changed[i]
        }'
    echo
    echo "### Regressions to act on"
    echo
    join -t"$(printf '\t')" \
        <(tail -n +2 "$pa" | cut -f1,2 | sort) \
        <(tail -n +2 "$pb" | cut -f1,2 | sort) \
    | awk -F'\t' '$2=="AGREED" && $3!="AGREED" {printf("- `%s` agreed last time and is now %s\n", $1, $3); n++}
                  END {if (n==0) print "None: nothing that agreed has stopped agreeing."}'
fi
echo

# ---------------------------------------------------------------------------
# Pause times: the tail, week over week
# ---------------------------------------------------------------------------
ga="$(locate "$PREV" gc-pauses samples.tsv)"
gb="$(locate "$CURR" gc-pauses samples.tsv)"
echo "## Collector pauses"
echo
if [ -z "$ga" ] || [ -z "$gb" ]; then
    echo "Absent from one side: previous=\`${ga:-none}\` current=\`${gb:-none}\`."
else
    pct() {
        awk -F'\t' -v c="$2" -v p="$3" '
            NR>1 && $c != "-" { v[++n] = $c + 0 }
            END {
                if (n == 0) { print "-"; exit }
                for (i=1;i<=n;i++) for (j=i+1;j<=n;j++) if (v[j]<v[i]) {t=v[i];v[i]=v[j];v[j]=t}
                k = int(p/100*n + 0.999999); if (k<1) k=1; if (k>n) k=n
                print v[k]
            }' "$1"
    }
    echo "| statistic | previous | current |"
    echo "|---|---|---|"
    echo "| longest pause per run, p50 (us) | $(pct "$ga" 8 50) | $(pct "$gb" 8 50) |"
    echo "| longest pause per run, p99 (us) | $(pct "$ga" 8 99) | $(pct "$gb" 8 99) |"
    echo "| longest pause observed (us) | $(pct "$ga" 8 100) | $(pct "$gb" 8 100) |"
    echo "| mean pause per run, p50 (us) | $(pct "$ga" 10 50) | $(pct "$gb" 10 50) |"
    echo "| wall clock per run, p50 (ms) | $(pct "$ga" 11 50) | $(pct "$gb" 11 50) |"
    echo "| wall clock per run, p99 (ms) | $(pct "$ga" 11 99) | $(pct "$gb" 11 99) |"
    echo
    echo "A tail that grows while p50 holds is the shape to look for: the mean"
    echo "hides it, and a budget is spent in the tail."
fi
echo

# ---------------------------------------------------------------------------
# Fuzz causes: new, gone, and still there
# ---------------------------------------------------------------------------
echo "## Fuzz causes"
echo
causes() {
    local dir="$1"
    find "$dir" -path '*/findings/*' -name 'stderr.txt' 2>/dev/null \
        | sed -e 's|/stderr.txt$||' -e 's|.*/findings/||' | sort -u
}
ca="$(causes "$PREV")"
cb="$(causes "$CURR")"
if [ -z "$ca" ] && [ -z "$cb" ]; then
    echo "Neither run recorded a fuzz finding."
else
    new="$(comm -13 <(echo "$ca") <(echo "$cb"))"
    gone="$(comm -23 <(echo "$ca") <(echo "$cb"))"
    both="$(comm -12 <(echo "$ca") <(echo "$cb"))"
    echo "- new this run: ${new:-none}"
    echo "- not seen this run: ${gone:-none}"
    echo "- in both: ${both:-none}"
    echo
    echo "A cause that is \"not seen\" is not fixed: a different seed explores"
    echo "different inputs. Only a replay of its own recorded input says that."
fi

# ---------------------------------------------------------------------------
# Soak counters, if a soak window wrote any
# ---------------------------------------------------------------------------
sa="$(locate "$PREV" soak counters.tsv)"
sb="$(locate "$CURR" soak counters.tsv)"
echo
echo "## Soak counters"
echo
if [ -z "$sa" ] || [ -z "$sb" ]; then
    echo "No soak window on one side: previous=\`${sa:-none}\` current=\`${sb:-none}\`."
else
    for f in "$sa" "$sb"; do
        echo "### \`$f\`"
        echo
        awk -F'\t' 'NR==1 {next}
            {rss[NR]=$3; fds[NR]=$4; n++
             if (NR==2) {first_rss=$3; first_fd=$4}
             last_rss=$3; last_fd=$4}
            END {
                printf("- samples: %d\n", n)
                printf("- RSS first -> last: %s -> %s KiB (%+.1f%%)\n", first_rss, last_rss, first_rss ? (last_rss-first_rss)*100.0/first_rss : 0)
                printf("- open descriptors first -> last: %s -> %s\n", first_fd, last_fd)
            }' "$f"
        echo
    done
    echo "Monotone creep is the finding; a single spike is not."
fi
