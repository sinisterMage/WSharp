#!/usr/bin/env bash
#
# Tests for `scripts/check-change-label.sh`.
#
# A CI gate that is wrong in the permissive direction passes every pull request
# and is believed, which is worse than not having it -- so the interesting cases
# here are the ones where the rule must *refuse*: no label at all, and two.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly UNDER_TEST="$SCRIPT_DIR/../check-change-label.sh"

failures=0
checks=0

# expect <name> <wanted-exit> <wanted-substring-in-output> -- <label lines...>
expect() {
  local name="$1" wanted_exit="$2" wanted_text="$3"
  shift 3
  [ "${1:-}" = "--" ] && shift

  local input="" label
  for label in "$@"; do
    input+="$label"$'\n'
  done

  local output status
  # Both streams, because a refusal is written to stderr and an acceptance to
  # stdout, and the test should not have to know which.
  output="$(printf '%s' "$input" | "$UNDER_TEST" 2>&1)"
  status=$?

  checks=$((checks + 1))

  if [ "$status" -ne "$wanted_exit" ]; then
    echo "FAIL $name: exit $status, wanted $wanted_exit"
    echo "     output: $output"
    failures=$((failures + 1))
    return
  fi

  if [ -n "$wanted_text" ] && [[ "$output" != *"$wanted_text"* ]]; then
    echo "FAIL $name: output did not mention '$wanted_text'"
    echo "     output: $output"
    failures=$((failures + 1))
    return
  fi

  echo "ok   $name"
}

# --- exactly one: accepted, and the message names which one ------------------

expect "breaking alone"  0 "change:breaking" -- "change:breaking"
expect "additive alone"  0 "change:additive" -- "change:additive"
expect "fix alone"       0 "change:fix"      -- "change:fix"
expect "none alone"      0 "change:none"     -- "change:none"

# --- none: refused, and the message says what to add ------------------------

expect "no labels at all"     1 "none of the change labels" --
expect "only other labels"    1 "Add exactly one of"        -- "bug" "P2" "area: stdlib"
expect "a near miss"          1 "none of the change labels" -- "change" "changes:fix" "change-fix"

# --- more than one: refused, and the message names them ---------------------

expect "two change labels"    1 "carries 2 change labels"   -- "change:fix" "change:additive"
expect "all four"             1 "carries 4 change labels"   -- \
  "change:breaking" "change:additive" "change:fix" "change:none"

# --- the four are matched exactly, not by prefix or substring ---------------

# `change:none` must not be found inside a longer label; a repository that later
# adds `change:none-of-the-above` should not have it silently satisfy the gate.
expect "longer label is not one of the four" 1 "none of the change labels" -- \
  "change:none-of-the-above"
expect "prefixed label is not one of the four" 1 "none of the change labels" -- \
  "proposed change:fix"

# --- the labels the repository actually carries do not interfere ------------

expect "one change label among many others" 0 "change:additive" -- \
  "area: stdlib" "P3" "change:additive" "good first issue" "needs-triage"

# --- whitespace and blank lines ---------------------------------------------

# `jq -r` on an empty array emits nothing; on a list it emits a trailing
# newline. Neither may be read as a label.
expect "blank lines are not labels" 1 "none of the change labels" -- "" "" ""
expect "surrounding whitespace is trimmed" 0 "change:fix" -- "  change:fix  "

# ---------------------------------------------------------------------------

echo
if [ "$failures" -eq 0 ]; then
  echo "$checks checks, all passed"
  exit 0
fi
echo "$checks checks, $failures failed"
exit 1
