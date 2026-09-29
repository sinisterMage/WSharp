#!/usr/bin/env bash
# Does `fuzz.pl` tell a compiler hang from a program that simply runs long?
#
# `fuzz.pl --target run` executes what the compiler produced. A valid program
# may legitimately never terminate -- an infinite loop, or a blocking call
# nothing answers. Reporting that as a compiler hang is the false positive that
# made the first two nightlies red (`net.accept` on a listener with no client,
# #52), and it recurs every night because `tests/cases/net_nonblocking.ws` is a
# seed.
#
# The fix is a differential: on a `run` timeout, `check` (which must answer
# every input) and `build` (which compiles but does not execute) are asked about
# the same bytes. Only a clean `build` exit 0 means the compiler finished and
# the program's own execution ran long. This selftest drives the **real**
# `fuzz.pl` over a stub compiler whose four answers are known in advance, and
# asserts each verdict and the exit status.
#
# It needs no cargo, no `cc` and no W# compiler, so it runs in the ordinary CI
# job in seconds. What it does *not* test: whether W# is correct. It tests that
# the harness can tell "the compiler hung" from "the program ran for ever",
# which is the property the gate rests on.
#
# Usage:
#   tests/harness/fuzz-selftest.sh

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-fuzz-selftest-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

failures=0
ok()  { printf 'ok    %s\n' "$*"; }
bad() { printf 'bad   %s\n' "$*"; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
# The stub compiler.
#
# Reads a marker out of the file it was handed and answers per verb, so a case
# can say exactly which of the four situations `check_input` must classify.
#
#   MARKER-frontend-too     run hangs, check hangs        -> HANG (compiler hung)
#   MARKER-valid-loop       run hangs, check+build exit 0  -> NONTERMINATING
#   MARKER-check-refuses    run hangs, check exits 1       -> HANG (run-only)
#   MARKER-build-hangs      run hangs, check 0, build hangs-> HANG (codegen)
#
# A hang is `sleep` long enough for `timeout -s KILL 1` to kill it.
# ---------------------------------------------------------------------------
cat >"$WORK/wsharp" <<'STUB'
#!/usr/bin/env bash
# Stub compiler for tests/harness/fuzz-selftest.sh. Not W#.
set -uo pipefail
verb="$1"; shift
while [ "${1:-}" = "--gc-stress" ]; do shift; done
file="$1"
m="$(grep -o 'MARKER-[a-z-]*' "$file" 2>/dev/null | head -1)"
case "$verb:$m" in
    run:MARKER-frontend-too|check:MARKER-frontend-too) sleep 30 ;;
    run:MARKER-valid-loop)                             sleep 30 ;;
    check:MARKER-valid-loop|build:MARKER-valid-loop)   exit 0 ;;
    run:MARKER-check-refuses)                          sleep 30 ;;
    check:MARKER-check-refuses) echo "error: nope" >&2; exit 1 ;;
    run:MARKER-build-hangs)                            sleep 30 ;;
    check:MARKER-build-hangs)                          exit 0 ;;
    build:MARKER-build-hangs)                          sleep 30 ;;
    *)                                                 exit 0 ;;
esac
STUB
chmod +x "$WORK/wsharp"

mkdir -p "$WORK/cases"
for pair in "frontend-too MARKER-frontend-too" \
            "valid-loop MARKER-valid-loop" \
            "check-refuses MARKER-check-refuses" \
            "build-hangs MARKER-build-hangs"; do
    set -- $pair
    printf '%s\n' "$2" >"$WORK/cases/$1.ws"
done

# ---------------------------------------------------------------------------
# Run the real fuzz.pl in replay mode, which classifies one file with no
# mutation and no reduction. `--timeout 1` so a `sleep 30` is a HANG quickly.
# ---------------------------------------------------------------------------
verdict_of() {
    WSHARP="$WORK/wsharp" TIMEOUT_BIN="${TIMEOUT_BIN:-timeout}" \
        perl "$HERE/fuzz.pl" --target run --timeout 1 --replay "$WORK/cases/$1.ws" 2>/dev/null \
        | sed -n 's/^verdict:   //p'
}
status_of() {
    WSHARP="$WORK/wsharp" TIMEOUT_BIN="${TIMEOUT_BIN:-timeout}" \
        perl "$HERE/fuzz.pl" --target run --timeout 1 --replay "$WORK/cases/$1.ws" >/dev/null 2>&1
    echo $?
}

expect_verdict() {
    local name="$1" want="$2" got
    got="$(verdict_of "$name")"
    if [ "$got" = "$want" ]; then
        ok "$name -> $want"
    else
        bad "$name -> got '${got:-<nothing>}', wanted '$want'"
    fi
}
expect_status() {
    local name="$1" want="$2" got
    got="$(status_of "$name")"
    if [ "$got" = "$want" ]; then
        ok "$name exits $want"
    else
        bad "$name exited $got, wanted $want"
    fi
}

command -v timeout >/dev/null 2>&1 || command -v gtimeout >/dev/null 2>&1 || {
    echo "fuzz-selftest: needs timeout(1) to bound the stub; none on PATH" >&2
    exit 2
}
if ! command -v timeout >/dev/null 2>&1; then TIMEOUT_BIN=gtimeout; fi

# `run` hangs and `check` hangs too: the compiler hung. Still a finding.
expect_verdict frontend-too  HANG
expect_status  frontend-too  1

# `run` hangs but `check` and `build` both exit 0: the program is valid and ran
# long. NOT a finding -- this is the #52 false positive.
expect_verdict valid-loop    NONTERMINATING
expect_status  valid-loop    0

# `run` executes a program `check` refuses: an anomaly in `run`, still a finding.
expect_verdict check-refuses HANG
expect_status  check-refuses 1

# `run` hangs and `build` (codegen, no execution) hangs too: a real compiler
# hang, still a finding.
expect_verdict build-hangs   HANG
expect_status  build-hangs   1

echo
if [ "$failures" -eq 0 ]; then
    echo "fuzz-selftest: the fuzzer tells a compiler hang from a program that ran long"
    exit 0
fi
echo "fuzz-selftest: $failures assertion(s) failed" >&2
exit 1
