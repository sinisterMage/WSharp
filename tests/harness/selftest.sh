#!/usr/bin/env bash
# Does the harness report what actually happened?
#
# `parity.sh` is a gate, and a gate nobody has exercised is not one. Its whole
# job is to tell four outcomes apart -- three modes agreeing, two modes
# disagreeing about stdout, two disagreeing about exit status, and one mode
# disagreeing with itself -- and the only way to know it can is to hand it
# cases whose answers are known in advance.
#
# So this builds a **stub compiler**: a shell script that reads directives out
# of a case file and prints what they say, per mode. Then it runs the real
# `parity.sh` over a small corpus of stub cases and asserts the verdict for each
# one. What is under test is the harness's comparison logic, not W#.
#
# **It needs no cargo, no `cc` and no W# compiler**, which is the point twice
# over: it runs in the ordinary CI job in seconds rather than in the nightly,
# and it runs on a machine that cannot build the language at all. A change that
# breaks `parity.sh`'s verdicts then fails on the pull request that made it,
# instead of surviving until a nightly whose divergence list is empty for the
# wrong reason.
#
# What it does *not* test: whether W# is correct, whether the real compiler's
# three modes agree, or anything about the collector. Those are what a real
# `parity.sh` run answers, and this cannot stand in for one.
#
# Usage:
#   tests/harness/selftest.sh

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-selftest-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

failures=0
ok()   { printf 'ok    %s\n' "$*"; }
bad()  { printf 'FAIL  %s\n' "$*"; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
# The stub compiler.
#
# Accepts the three command lines `parity.sh` issues -- `run FILE`,
# `run --gc-stress FILE`, and `build FILE -o EXE` -- and answers from
# directives in the case file:
#
#   //@run    out=<text> exit=<n>     what `wsharp run` should do
#   //@stress out=<text> exit=<n>     ... `wsharp run --gc-stress`
#   //@build  out=<text> exit=<n>     ... the built executable
#   //@<mode> random                  print a different number every time
#   //@<mode> gcstats                 also print a `W# gc:` line, as the real
#                                     compiler does under WSHARP_GC_STATS
#
# A mode with no directive of its own falls back to `//@run`, so a case that
# means "all three agree" is written once.
# ---------------------------------------------------------------------------
cat >"$WORK/wsharp" <<'STUB'
#!/usr/bin/env bash
# Stub compiler for tests/harness/selftest.sh. Not W#.
set -uo pipefail

verb="$1"; shift
mode=run
case "$verb" in
    run)
        if [ "${1:-}" = "--gc-stress" ]; then mode=stress; shift; fi
        file="$1"
        ;;
    build)
        file="$1"; shift
        # `-o EXE`: write a script that replays the `build` directive, which is
        # what `parity.sh` then executes.
        [ "${1:-}" = "-o" ] && { exe="$2"; }
        printf '#!/usr/bin/env bash\nexec %q run --stub-mode-build %q\n' "$0" "$file" >"$exe"
        chmod +x "$exe"
        exit 0
        ;;
    *) echo "stub: unknown verb $verb" >&2; exit 2 ;;
esac
# The built executable re-enters here asking for the `build` directive.
if [ "${file:-}" = "--stub-mode-build" ]; then mode=build; file="$2"; fi
[ "${1:-}" = "--stub-mode-build" ] && { mode=build; file="$2"; }

directive() {
    sed -n "s|^//@$1 ||p" "$file" | head -1
}
d="$(directive "$mode")"
[ -n "$d" ] || d="$(directive run)"

case "$d" in
    *random*)
        # A different answer every time, which is what a case that prints a
        # port number or a temporary path looks like to the harness.
        echo "value $RANDOM$RANDOM"
        exit 0
        ;;
esac
case "$d" in
    *gcstats*)
        # Noise `lib.sh::normalise` is supposed to remove. If it stops removing
        # it, this case turns into a false divergence and the selftest says so.
        echo "W# gc: 3 collections, 1 trace, longest pause 0.4ms"
        ;;
esac

out="$(printf '%s\n' "$d" | sed -n 's|.*out=\([^ ]*\).*|\1|p')"
code="$(printf '%s\n' "$d" | sed -n 's|.*exit=\([0-9]*\).*|\1|p')"
[ -n "$out" ] && echo "$out"
exit "${code:-0}"
STUB
chmod +x "$WORK/wsharp"

# ---------------------------------------------------------------------------
# The corpus, one case per verdict the harness must be able to reach.
# ---------------------------------------------------------------------------
CASES="$WORK/cases"
mkdir -p "$CASES"

# All three modes say the same thing.
cat >"$CASES/agree.ws" <<'EOF'
//@run out=hello exit=0
EOF

# The built program prints something the JIT did not. This is the divergence
# the AOT backend exists to be checked for.
cat >"$CASES/diverge_stdout.ws" <<'EOF'
//@run out=hello exit=0
//@build out=goodbye exit=0
EOF

# Stress collects at every allocation and, here, exits differently. An exit
# status disagreement must be caught before stdout is even looked at.
cat >"$CASES/diverge_exit.ws" <<'EOF'
//@run out=hello exit=0
//@stress out=hello exit=3
EOF

# A case that does not agree with itself. It must be reported as
# NONDETERMINISTIC rather than as a divergence between the modes: the finding
# is about determinism and has a different owner.
cat >"$CASES/nondet.ws" <<'EOF'
//@run random
//@build out=fixed exit=0
EOF

# The collector's statistics line differs between modes for reasons that are
# not a defect, and `normalise` removes it. If that stops working, every
# gc case in the real corpus becomes a false divergence -- so pin it here,
# where it costs a second rather than an hour.
cat >"$CASES/gcstats_noise.ws" <<'EOF'
//@run out=hello exit=0
//@stress gcstats out=hello exit=0
EOF

# ---------------------------------------------------------------------------
# Run the real parity.sh over it.
# ---------------------------------------------------------------------------
OUT="$WORK/report"
WSHARP="$WORK/wsharp" CASES="$CASES" WSHARP_HARNESS_REPORTS="$OUT" \
    bash "$HERE/parity.sh" --timeout 30 >"$WORK/parity.log" 2>&1
parity_status=$?

TSV="$OUT/results.tsv"
if [ ! -f "$TSV" ]; then
    echo "selftest: parity.sh wrote no results.tsv; its output was:" >&2
    cat "$WORK/parity.log" >&2
    exit 1
fi

verdict() { awk -F'\t' -v c="$1" '$1==c {print $2}' "$TSV"; }
detail()  { awk -F'\t' -v c="$1" '$1==c {print $4}' "$TSV"; }

expect() {
    local name="$1" want="$2" got
    got="$(verdict "$name")"
    if [ "$got" = "$want" ]; then
        ok "$name -> $want"
    else
        bad "$name -> got '${got:-<no row>}', wanted '$want'"
    fi
}

expect agree          AGREED
expect diverge_stdout DIVERGED
expect diverge_exit   DIVERGED
expect nondet         NONDETERMINISTIC
expect gcstats_noise  AGREED

# The detail column is what a reader acts on, so it is part of the contract:
# "something diverged" without naming the mode and the stream is not a finding.
case "$(detail diverge_stdout)" in
    *run-vs-build*stdout*) ok   "diverge_stdout names the mode and the stream" ;;
    *) bad "diverge_stdout detail was '$(detail diverge_stdout)', wanted run-vs-build(stdout)" ;;
esac
case "$(detail diverge_exit)" in
    *run-vs-stress*exit*) ok   "diverge_exit names the mode and the exit status" ;;
    *) bad "diverge_exit detail was '$(detail diverge_exit)', wanted run-vs-stress(exit status)" ;;
esac

# The outputs of a divergence are the evidence a defect is filed from. A report
# that named a divergence and kept nothing would cost the fixer the reduction.
if [ -s "$OUT/diffs/diverge_stdout.run-vs-build.txt" ]; then
    ok "diverge_stdout kept both outputs under diffs/"
else
    bad "diverge_stdout kept no diff under $OUT/diffs/"
fi

# A run with divergences must not exit 0, or the CI job it gates passes.
if [ "$parity_status" -ne 0 ]; then
    ok "parity.sh exits non-zero when cases diverged (was $parity_status)"
else
    bad "parity.sh exited 0 despite two diverged cases"
fi

# And the inverse: a clean corpus must exit 0, or the gate can never be met.
CLEAN="$WORK/clean"; mkdir -p "$CLEAN"
cp "$CASES/agree.ws" "$CASES/gcstats_noise.ws" "$CLEAN/"
WSHARP="$WORK/wsharp" CASES="$CLEAN" WSHARP_HARNESS_REPORTS="$WORK/report-clean" \
    bash "$HERE/parity.sh" --timeout 30 >"$WORK/parity-clean.log" 2>&1
if [ $? -eq 0 ]; then
    ok "parity.sh exits 0 when every case agreed"
else
    bad "parity.sh exited non-zero on a corpus with no divergence"
    cat "$WORK/parity-clean.log" >&2
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "selftest: the harness reports what happened"
    exit 0
fi
echo "selftest: $failures assertion(s) failed" >&2
exit 1
