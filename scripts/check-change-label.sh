#!/usr/bin/env bash
#
# Criterion 2's mechanism: every pull request carries exactly one of
# `change:breaking`, `change:additive`, `change:fix`, `change:none`.
#
# This is what makes "was there a breaking change in the window" a query rather
# than a memory, and it is why the rule has to be enforced *before* day 0 -- a
# window whose merges were not labelled cannot be checked after the fact, so a
# rule that starts late leaves a hole no later work can fill.
#
# The decision lives in a script rather than in an `if` inside the workflow so
# that it has tests (`scripts/tests/check-change-label.test.sh`): a gate whose
# own logic is unverified is a gate that can pass everything and be believed.
#
# Reads label names on stdin, one per line, and says nothing about labels
# outside the four. Exit 0 means exactly one was found.

set -uo pipefail

readonly LABELS=(
  "change:breaking"
  "change:additive"
  "change:fix"
  "change:none"
)

# The four, rendered for a message. Written once so the help text cannot drift
# from the set actually being matched.
label_list() {
  local out="" l
  for l in "${LABELS[@]}"; do
    [ -n "$out" ] && out+=", "
    out+="$l"
  done
  echo "$out"
}

is_change_label() {
  local candidate="$1" known
  for known in "${LABELS[@]}"; do
    [ "$candidate" = "$known" ] && return 0
  done
  return 1
}

main() {
  local -a found=()
  local line

  # `read -r` without a trailing-newline guard would drop a last line that has
  # none, which is exactly what `jq -r` produces for an empty array versus a
  # one-element one.
  while IFS= read -r line || [ -n "$line" ]; do
    # Trim surrounding whitespace: a label may legitimately contain a space
    # inside it, but never at either end, and a hand-written test fixture
    # indents.
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [ -z "$line" ] && continue
    if is_change_label "$line"; then
      found+=("$line")
    fi
  done

  case "${#found[@]}" in
    1)
      echo "change label: ${found[0]}"
      return 0
      ;;
    0)
      echo "error: this pull request carries none of the change labels." >&2
      echo "       Add exactly one of: $(label_list)" >&2
      echo "       The definition of a breaking change is in RELEASE-CRITERIA-1.0.md," >&2
      echo "       under \"What counts as a breaking change\"." >&2
      return 1
      ;;
    *)
      echo "error: this pull request carries ${#found[@]} change labels: ${found[*]}" >&2
      echo "       Exactly one is required. Remove the ones that do not apply." >&2
      return 1
      ;;
  esac
}

main "$@"
