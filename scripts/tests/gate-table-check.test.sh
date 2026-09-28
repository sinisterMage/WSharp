#!/usr/bin/env bash
#
# Tests for `scripts/gate-table-check.sh`.
#
# This check exists because a hand-maintained table went stale in twenty minutes,
# so the case that matters is the one where it must *refuse*: a row claiming a
# file that is not there, and a row claiming a file is missing when somebody has
# written it. A drift check that only ever passes is the drift it was meant to
# catch, one level up.
#
# Each case builds a throwaway tree with a two-row table in it, so the fixtures
# say what they test and no case depends on the real document.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly UNDER_TEST="$SCRIPT_DIR/../gate-table-check.sh"

failures=0
checks=0

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# fixture <dir> <row...> -- writes a document with a stamp and the given rows.
fixture() {
  local dir="$work/$1"
  shift
  mkdir -p "$dir"
  {
    echo "## The state of the checks"
    echo
    echo '**Read against `main` at `41e4d70`.**'
    echo
    echo "| Check | Gate | State | Owed by |"
    echo "|---|---|---|---|"
    local row
    for row in "$@"; do
      echo "$row"
    done
    echo
    echo "Prose after the table, which the reader stops at."
  } >"$dir/CRITERIA.md"
  echo "$dir"
}

# expect <name> <dir> <wanted-exit> <wanted-substring> [document]
expect() {
  local name="$1" dir="$2" wanted_exit="$3" wanted_text="$4" doc="${5:-CRITERIA.md}"
  local output status

  # Both streams: a refusal goes to stderr and an agreement to stdout, and the
  # test should not have to know which.
  output="$(bash "$UNDER_TEST" "$doc" "$dir" 2>&1)"
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

# --- a table that agrees with its tree --------------------------------------

d="$(fixture agree \
  '| `scripts/there.sh` | 1 | **written** (#20) | Dex |' \
  '| `scripts/absent.sh` | 2 | **not written** | Ash |')"
mkdir -p "$d/scripts" && : >"$d/scripts/there.sh"
expect "both rows true" "$d" 0 "2 of 2 rows"

# --- the drift in each direction --------------------------------------------

d="$(fixture claims_missing \
  '| `scripts/ghost.sh` | 1 | **written** (#20) | Dex |')"
expect "claims a file that is not there" "$d" 1 "does not exist"

d="$(fixture missed_arrival \
  '| `scripts/arrived.sh` | 3 | **not written** | Ash |')"
mkdir -p "$d/scripts" && : >"$d/scripts/arrived.sh"
expect "missed an arrival" "$d" 1 "is not written, and it is there"

# --- a row with no verdict is a row nothing can check -----------------------

d="$(fixture no_verdict \
  '| `scripts/there.sh` | 1 | being thought about | Dex |')"
mkdir -p "$d/scripts" && : >"$d/scripts/there.sh"
expect "no verdict" "$d" 1 "states no verdict"

# --- "not written" contains "written", and must not be read as it -----------

d="$(fixture not_written_prefix \
  '| `scripts/ghost.sh` | 4 | **not written.** The data it needs is on `main` | Ash |')"
expect "a full stop inside the bold run" "$d" 0 "1 of 1 rows"

# --- workflow jobs ----------------------------------------------------------

d="$(fixture job_present \
  '| `scripts/there.sh` | 6d | **written** (#21), as `the-job` in `.github/workflows/gates.yml` | Ash |')"
mkdir -p "$d/scripts" "$d/.github/workflows"
: >"$d/scripts/there.sh"
printf 'name: Gates\njobs:\n  the-job:\n    runs-on: ubuntu-latest\n' >"$d/.github/workflows/gates.yml"
expect "a job that exists" "$d" 0 "1 of 1 rows"

d="$(fixture job_renamed \
  '| `scripts/there.sh` | 6d | **written** (#21), as `old-name` in `.github/workflows/gates.yml` | Ash |')"
mkdir -p "$d/scripts" "$d/.github/workflows"
: >"$d/scripts/there.sh"
printf 'name: Gates\njobs:\n  new-name:\n    runs-on: ubuntu-latest\n' >"$d/.github/workflows/gates.yml"
expect "a job that was renamed" "$d" 1 "has no \`old-name\` job"

# A `run:` line mentioning the name must not satisfy the claim: the point of
# checking a job name is that a reader can find it in a CI run.
d="$(fixture job_only_in_run \
  '| `scripts/there.sh` | 6d | **written** (#21), as `the-job` in `.github/workflows/gates.yml` | Ash |')"
mkdir -p "$d/scripts" "$d/.github/workflows"
: >"$d/scripts/there.sh"
printf 'name: Gates\njobs:\n  other:\n    steps:\n      - run: echo "  the-job: not a job"\n' \
  >"$d/.github/workflows/gates.yml"
expect "a name that is only in a run line" "$d" 1 "has no \`the-job\` job"

d="$(fixture jobs_listed \
  '| `scripts/there.sh` | 5 | **written**, as `a`, `b` and `c` in `.github/workflows/gates.yml` | Dex |')"
mkdir -p "$d/scripts" "$d/.github/workflows"
: >"$d/scripts/there.sh"
printf 'name: Gates\njobs:\n  a:\n    x: 1\n  b:\n    x: 1\n' >"$d/.github/workflows/gates.yml"
expect "the third of three jobs is missing" "$d" 1 "has no \`c\` job"

# --- another repository's path ----------------------------------------------

d="$(fixture external_named \
  '| `tests/rungs.sh` | 7 | **not written.** It belongs to `sinisterMage/sharpie`, not to this repository | Ash |')"
expect "an external path, whose owner is named" "$d" 0 "cannot check its state"

d="$(fixture external_unnamed \
  '| `tests/rungs.sh` | 7 | **not written** | Ash |')"
expect "an external path, whose owner is not" "$d" 1 "and this row does not say so"

# Both verdicts are accepted on an external path, and neither is believed. The
# first version of this script refused **written** here, which encoded "sharpie
# has not written it yet" into a check about *this* tree -- and sharpie wrote it
# the same day, so the check then stood in the way of the table being right.
d="$(fixture external_written \
  '| `tests/rungs.sh` | 7 | **written**, in `sinisterMage/sharpie` | Ash |')"
expect "an external path said to be written elsewhere" "$d" 0 "cannot check its state"

# It must still be unchecked rather than quietly passed -- the number of rows
# held to the tree is what says so.
expect "an external row counts as unchecked" "$d" 0 "0 of 1 rows"

# --- a row this check cannot hold to anything, reported rather than passed ---

d="$(fixture unnameable \
  '| a committed baseline per triple | 8 | **not written.** No baseline exists | Ridge |')"
expect "a row naming no path" "$d" 0 "names no path in this tree"

# --- the stamp --------------------------------------------------------------

d="$work/unstamped"
mkdir -p "$d"
{
  echo "| Check | Gate | State | Owed by |"
  echo "|---|---|---|---|"
  echo '| `scripts/ghost.sh` | 1 | **not written** | Ash |'
} >"$d/CRITERIA.md"
expect "no commit stamp" "$d" 1 "carries no"

# --- the document, and the table, have to be there at all -------------------

d="$work/empty"
mkdir -p "$d"
expect "no document" "$d" 2 "does not exist"

d="$work/tableless"
mkdir -p "$d"
printf '# A document\n\nWith no state table in it.\n' >"$d/CRITERIA.md"
expect "no table" "$d" 2 "found no state table"

# --- the real document, against the real tree ------------------------------
#
# The cases above prove the logic; this proves the thing the logic is for. It is
# last so that a failure here is read as "the table drifted" rather than as "the
# check is broken".
repo="$(cd "$SCRIPT_DIR/../.." && pwd)"
expect "RELEASE-CRITERIA-1.0.md against this tree" "$repo" 0 "agree" "RELEASE-CRITERIA-1.0.md"

echo
if [ "$failures" -gt 0 ]; then
  echo "$failures of $checks checks failed."
  exit 1
fi
echo "$checks checks passed."
