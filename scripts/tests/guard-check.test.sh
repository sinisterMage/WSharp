#!/usr/bin/env bash
#
# Tests for `scripts/guard-check.sh`.
#
# Criterion 3's data is GitHub's -- which issues closed, and what closed them --
# so every case here drives the check through a stub fetcher (`GUARD_FETCH`)
# that answers from files, and the live gate runs separately. Two halves, as
# `followups-check.test.sh` has: the check must refuse an unguarded fix, and it
# must answer 2 rather than 0 whenever it could not ask.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly UNDER_TEST="$SCRIPT_DIR/../guard-check.sh"

failures=0
checks=0

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The stub: `issues` prints $STUB/issues, `changes N` prints $STUB/changes-N,
# and a file named fail-<mode>[-N] makes that question fail.
stub="$work/stub.sh"
cat >"$stub" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  issues) [ -e "$STUB/fail-issues" ] && exit 1; cat "$STUB/issues" ;;
  changes) [ -e "$STUB/fail-changes-$2" ] && exit 1; cat "$STUB/changes-$2" ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$stub"

# scenario NAME -- a fresh stub directory; rows are written into it by the case.
scenario() {
  STUB="$work/$1"
  mkdir -p "$STUB"
  : >"$STUB/issues"
}
issue() { printf '%s\t%s\t%s\t%s\n' "$1" 2026-10-01 "$2" "$3" >>"$STUB/issues"; }
changes() {
  local n="$1"
  shift
  printf '%s\n' "$@" >"$STUB/changes-$n"
}

# expect NAME STATUS PATTERN [ARGS...] -- run the check and hold it to the exit
# status and to a line of its output.
expect() {
  local name="$1" want="$2" pattern="$3" out status
  shift 3
  checks=$((checks + 1))
  out="$(cd "$work" && STUB="$STUB" GUARD_FETCH="$stub" GUARD_REPO="${REPO:-sinisterMage/WSharp}" \
    FREEZE_START_FILE="$work/no-such-file" bash "$UNDER_TEST" "$@" 2>&1)"
  status=$?
  if [ "$status" -ne "$want" ] || ! grep -qE -- "$pattern" <<<"$out"; then
    failures=$((failures + 1))
    printf 'FAIL  %s: wanted exit %s and /%s/, got exit %s:\n%s\n' "$name" "$want" "$pattern" "$status" "$out"
  else
    printf 'ok    %s\n' "$name"
  fi
}

T=$'\t'

scenario none
expect "no window at all cannot be answered" 2 "no freeze start declared"
expect "a malformed --since cannot be answered" 2 "YYYY-MM-DD" --since yesterday

scenario empty
expect "an empty window is met, and says how many it checked" 0 "MET: 0 defect" --since 2026-09-26

scenario cases
issue 101 completed "bug,P1,area: codegen"
changes 101 "pr 7" "crates/wsharp-codegen/src/lower.rs$T-" "tests/cases/panic_x.ws$T-"
expect "a codegen fix with a case is guarded" 0 "#101 guarded by #7: tests/cases/panic_x.ws" --since 2026-09-26

scenario unguarded
issue 102 completed "bug,P2,area: sema"
changes 102 "pr 8" "crates/wsharp-sema/src/infer.rs$T-" "CLAUDE.md$T-"
expect "a sema fix with no guard is named" 1 "FAIL  #102 .*area: sema.* fixed by #8, which touches no guard" --since 2026-09-26

scenario unit
issue 103 completed "bug,area: runtime"
changes 103 "pr 9" "crates/wsharp-runtime/src/heap.rs${T}test"
expect "a runtime fix that adds a #[test] beside the code is guarded" 0 "#103 guarded by #9: crates/wsharp-runtime/src/heap.rs" --since 2026-09-26

scenario unit-without-test
issue 104 completed "bug,area: runtime"
changes 104 "pr 10" "crates/wsharp-runtime/src/heap.rs$T-"
expect "a runtime fix that only edits source is not" 1 "FAIL  #104" --since 2026-09-26

scenario crate-tests
issue 105 completed "bug,area: cli"
changes 105 "pr 11" "crates/wsharp-cli/src/main.rs$T-" "crates/wsharp-cli/tests/verbs.rs$T-"
expect "a cli fix with a crate test is guarded" 0 "#105 guarded" --since 2026-09-26

scenario workflow-for-codegen
issue 106 completed "bug,area: codegen"
changes 106 "pr 12" "crates/wsharp-codegen/src/lib.rs$T-" ".github/workflows/ci.yml$T-"
expect "a CI job is not a guard for a codegen defect" 1 "FAIL  #106" --since 2026-09-26

scenario workflow-no-area
issue 107 completed "bug"
changes 107 "pr 13" "scripts/verify-install.sh$T-" "scripts/tests/verify-install.test.sh$T-"
expect "a script's own test guards a defect with no area" 0 "#107 guarded by #13: scripts/tests/verify-install.test.sh" --since 2026-09-26

scenario closed-by-nothing
issue 108 completed "defect"
changes 108 "none"
expect "an issue nothing closed is unguarded" 1 "FAIL  #108 .* closed by no pull request" --since 2026-09-26

scenario skipped
issue 109 not_planned "bug"
issue 110 completed "bug,v1.0-limitation"
expect "not planned and limitations are not criterion 3's" 0 "MET: 0 defect" --since 2026-09-26

scenario several-prs
issue 111 completed "bug,area: stdlib"
changes 111 "pr 14" "crates/wsharp-runtime/src/std/toml.ws$T-" "pr 15" "tests/cases/toml_depth.ws$T-"
expect "a hand-closed issue is guarded if any merged pull request guards it" 0 "#111 guarded by #14 #15" --since 2026-09-26

scenario one-bad-one-good
issue 112 completed "bug,area: codegen"
issue 113 completed "bug,area: codegen"
changes 112 "pr 16" "tests/cases/a.ws$T-"
changes 113 "pr 17" "crates/wsharp-codegen/src/lower.rs$T-"
expect "one unguarded fix fails the gate however many are guarded" 1 "NOT MET" --since 2026-09-26

REPO=sinisterMage/sharpie
scenario sharpie
issue 1 completed "bug"
changes 1 "pr 3" "src/toolchain.ws$T-" "tests/toolchain.ws$T-"
expect "in sharpie, tests/ is the guard" 0 "#1 guarded by #3: tests/toolchain.ws" --since 2026-09-26
scenario sharpie-unguarded
issue 2 completed "bug"
changes 2 "pr 4" "src/toolchain.ws$T-" "README.md$T-"
expect "in sharpie, a source-only fix is not" 1 "FAIL  #2" --since 2026-09-26
unset REPO

# --- the half a green run never shows: not being able to ask ----------------

scenario fail-list
touch "$STUB/fail-issues"
expect "a failed issue list is 2, never 0" 2 "could not list" --since 2026-09-26

scenario fail-changes
issue 114 completed "bug"
touch "$STUB/fail-changes-114"
expect "a failed read of the closing change is 2, never 0" 2 "could not read what closed #114" --since 2026-09-26

checks=$((checks + 1))
if out="$(cd "$work" && env -u GITHUB_TOKEN -u GH_TOKEN -u GUARD_FETCH PATH="$PATH" \
  bash "$UNDER_TEST" --since 2026-09-26 2>&1)"; then
  failures=$((failures + 1))
  printf 'FAIL  no token: exited 0\n%s\n' "$out"
elif [ $? -ne 2 ]; then
  failures=$((failures + 1))
  printf 'FAIL  no token: wanted exit 2\n%s\n' "$out"
else
  printf 'ok    no token is 2, never 0\n'
fi

printf '\n%s of %s checks passed\n' "$((checks - failures))" "$checks"
[ "$failures" -eq 0 ]
