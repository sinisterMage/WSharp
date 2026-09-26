#!/usr/bin/env bash
#
# Asserts RELEASE-CRITERIA-1.0.md's "The state of the checks" table against the
# tree it describes.
#
# That table is the document's own account of which gate checks exist, and it is
# what anybody reads to decide what is still owed before v1.0. Revision 2 of it
# was accurate when it was published and wrong on six of ten rows twenty minutes
# later, because three pull requests landed behind it -- so every wrong row was
# either work somebody would redo or work nobody would pick up. A hand-maintained
# inventory of which files exist cannot be kept right by care; it needs a check.
#
# This is the same trick gate 6a plays on the prose: read what the document
# claims, and hold it to the tree.
#
# What is asserted, per row:
#
#   * A row whose State begins **written** -- every path it names exists.
#   * A row whose State says **not written** -- none of them does. That half
#     matters as much: a row still saying "not written" after the script arrives
#     is how somebody comes to write it twice.
#   * Every workflow job the row claims -- named in the regular form
#     "as `job`, `job` and `job` in `.github/workflows/file.yml`" -- is a key in
#     that workflow.
#   * A row that names a path this repository does not own has to say whose it
#     is, so the exemption cannot outlive its reason.
#
# What is not asserted: whether a written check *passes*. That is the gate's own
# job, and the table says so.
#
# Usage: gate-table-check.sh [document] [tree-root]

set -uo pipefail

readonly DOC_DEFAULT="RELEASE-CRITERIA-1.0.md"
readonly TABLE_HEADER="| Check | Gate | State | Owed by |"

# Paths named by the table that live in another repository. `tests/rungs.sh` is
# sharpie's: criterion 7 requires it, and nothing in this tree will ever show it
# arriving. One entry, and the row has to name the repository -- an exemption
# whose justification is checked is an exemption that can be retired.
readonly EXTERNAL_PATHS=("tests/rungs.sh")
readonly EXTERNAL_OWNER="sinisterMage/sharpie"

fail_count=0
row_count=0
unchecked_count=0

problem() {
  echo "error: $*" >&2
  fail_count=$((fail_count + 1))
}

is_external() {
  local candidate="$1" known
  for known in "${EXTERNAL_PATHS[@]}"; do
    [ "$candidate" = "$known" ] && return 0
  done
  return 1
}

# Every `backticked` token in a string, one per line.
backticked() {
  # `grep -o` over the whole cell rather than a bash loop: a cell holds a dozen
  # of these and the loop was the only slow part of this script.
  printf '%s\n' "$1" | grep -o '`[^`]*`' | tr -d '`'
}

# A token is a path in this tree if it has a directory and an extension. That is
# what separates `scripts/guard-check.sh` from `sinisterMage/sharpie` (no
# extension) and from `8451ee8` (no directory), both of which the State cells
# also render in backticks.
looks_like_path() {
  case "$1" in
    */*.*) return 0 ;;
    *) return 1 ;;
  esac
}

# The jobs a State cell claims, as "workflow<TAB>job" lines. The phrasing is
# fixed rather than free because this reads it: "as `a`, `b` and `c` in
# `.github/workflows/f.yml`".
claimed_jobs() {
  local cell="$1"
  local workflow token names job

  # Two string operations rather than one regular expression, because the list
  # may be "`a`, `b` and `c`" and a regexp that reads that is one nobody can
  # check by eye. Everything between the last "as " and " in `workflow`" is the
  # list, and the backticked tokens in it are the job names.
  while IFS= read -r token; do
    case "$token" in *.yml) ;; *) continue ;; esac
    workflow="$token"
    [[ "$cell" != *" in \`$workflow\`"* ]] && continue

    names="${cell%%" in \`$workflow\`"*}"
    [[ "$names" != *" as "* ]] && continue
    names="${names##*" as "}"

    while IFS= read -r job; do
      [ -n "$job" ] && printf '%s\t%s\n' "$workflow" "$job"
    done < <(backticked "$names")
  done < <(backticked "$cell" | sort -u)
}

check_row() {
  local check="$1" gate="$2" state="$3"
  local expect

  # "not written" is tested first because it contains the other. The bold run may
  # end in a full stop -- several rows have a sentence of explanation inside it --
  # so neither pattern requires the closing asterisks.
  case "$state" in
    *'**not written'*) expect="absent" ;;
    *'**written'*) expect="present" ;;
    *)
      problem "gate $gate: this row states no verdict."
      echo "       Its State must begin \`**written**\` or \`**not written**\`," >&2
      echo "       which is what makes the row checkable at all." >&2
      return
      ;;
  esac

  local named=0 path
  while IFS= read -r path; do
    looks_like_path "$path" || continue
    named=$((named + 1))

    if is_external "$path"; then
      if [ "$expect" = "present" ]; then
        problem "gate $gate: \`$path\` is not this repository's, so this row cannot claim it is written."
      elif [[ "$state" != *"$EXTERNAL_OWNER"* ]]; then
        problem "gate $gate: \`$path\` belongs to $EXTERNAL_OWNER, and this row does not say so."
        echo "       A reader would otherwise look for it here and conclude the table is wrong." >&2
      fi
      continue
    fi

    if [ -e "$path" ]; then
      if [ "$expect" = "absent" ]; then
        problem "gate $gate: the table says \`$path\` is not written, and it is there."
        echo "       Somebody wrote it and the table did not notice. Mark the row" >&2
        echo "       \`**written**\`, say which pull request wrote it, and re-stamp the commit." >&2
      fi
    else
      if [ "$expect" = "present" ]; then
        problem "gate $gate: the table says \`$path\` is written, and it does not exist."
        echo "       Either the path is wrong or the row is. A row claiming a check" >&2
        echo "       that is not there is a gate nobody can fail." >&2
      fi
    fi
  done < <(backticked "$check"; backticked "$state")

  local claims=0 workflow job
  while IFS=$'\t' read -r workflow job; do
    [ -z "$workflow" ] && continue
    claims=$((claims + 1))
    if [ ! -e "$workflow" ]; then
      problem "gate $gate: this row claims a job in \`$workflow\`, which does not exist."
      continue
    fi
    if ! workflow_has_job "$workflow" "$job"; then
      problem "gate $gate: \`$workflow\` has no \`$job\` job, which this row says it does."
      echo "       Job names are how a reader finds the gate in a CI run, so a wrong" >&2
      echo "       one is worse than none." >&2
    fi
  done < <(claimed_jobs "$state")

  # A row naming neither a path nor a job is one this check cannot hold to
  # anything, and criterion 8's baselines are legitimately such a row: no path
  # for them has been decided, so the table cannot name one. Reported rather
  # than passed silently, because "12 of 13 rows are checked" is the number a
  # reader of this output needs.
  if [ "$named" -eq 0 ] && [ "$claims" -eq 0 ]; then
    unchecked_count=$((unchecked_count + 1))
    echo "note: gate $gate: \"$check\" names no path in this tree, so only its wording is checked."
  fi
}

# The job keys of a workflow: two-space-indented keys under the top-level
# `jobs:`. Restricted to that block because `on:`, `permissions:` and
# `concurrency:` have keys at the same indentation, and a `run:` line mentioning
# a job's name would satisfy a bare grep.
workflow_has_job() {
  awk -v want="$2" '
    /^jobs:/ { in_jobs = 1; next }
    /^[^ #]/ { in_jobs = 0 }
    in_jobs && $0 ~ "^  " want ":" { found = 1 }
    END { exit(found ? 0 : 1) }
  ' "$1"
}

main() {
  local doc="${1:-$DOC_DEFAULT}"
  local root="${2:-.}"

  cd "$root" || {
    echo "error: cannot read the tree at $root" >&2
    return 2
  }

  if [ ! -f "$doc" ]; then
    echo "error: $doc does not exist." >&2
    echo "       This check reads the release criteria's own state table; without" >&2
    echo "       the document there is nothing to hold to the tree." >&2
    return 2
  fi

  # The stamp. The table claims which files exist, so it carries the commit it
  # was true at; a table with no stamp is one a reader cannot date.
  if ! grep -qE '^\*\*Read against `main` at `[0-9a-f]{7,40}`' "$doc"; then
    problem "$doc's state table carries no \`Read against \`main\` at <commit>\` stamp."
    echo "       A claim about which files exist needs the commit it was read at." >&2
  fi

  local in_table=0 line
  while IFS= read -r line; do
    if [ "$in_table" -eq 0 ]; then
      [ "$line" = "$TABLE_HEADER" ] && in_table=1
      continue
    fi
    # The table ends at the first line that is not a row.
    case "$line" in
      '|---'*) continue ;;
      '|'*) ;;
      *) break ;;
    esac

    local body="${line#|}"
    local check gate state
    IFS='|' read -r check gate state _ <<<"$body"
    check="$(echo "$check" | sed 's/^ *//;s/ *$//')"
    gate="$(echo "$gate" | sed 's/^ *//;s/ *$//')"
    state="$(echo "$state" | sed 's/^ *//;s/ *$//')"

    row_count=$((row_count + 1))
    check_row "$check" "$gate" "$state"
  done <"$doc"

  if [ "$row_count" -eq 0 ]; then
    echo "error: found no state table in $doc." >&2
    echo "       Expected a row of \"$TABLE_HEADER\"." >&2
    return 2
  fi

  if [ "$fail_count" -gt 0 ]; then
    echo >&2
    echo "$fail_count of $row_count rows in $doc's state table disagree with the tree." >&2
    echo "Fix the table, not this check: the tree is the fact." >&2
    return 1
  fi

  echo "$doc: $((row_count - unchecked_count)) of $row_count rows in the state table were held to the tree, and agree."
  return 0
}

main "$@"
