#!/usr/bin/env bash
#
# Answers criterion 4 of RELEASE-CRITERIA-1.0.md: is every remaining limitation
# resolved or documented?
#
# The criterion's rule, in its own words: each tracked limitation "has a GitHub
# issue that is either closed as fixed, or closed with the label
# `v1.0-limitation` **and** a section in `LIMITATIONS.md` naming it, stating what
# does not work, why it is not fixed for 1.0, and what would fix it. An issue
# left open is a failure; so is a closed one with no `LIMITATIONS.md` section."
#
# So there are two states a subject may be in and no third one, and this script
# asserts one of them per issue, by number.
#
# Exit status is the answer, so this composes:
#
#   0  every subject is resolved or documented
#   1  one is neither, and the reasons are on stdout
#   2  the question cannot be answered -- no `curl`, no `jq`, no network, no
#      token, a rate limit, a document that is not there
#
# **Two and one are deliberately different, and two is the point of this
# script's shape.** Every other release gate in this tree reads only the tree;
# this one has to ask GitHub whether an issue is open, which is a question that
# can fail for reasons that say nothing at all about the criterion. A gate that
# answered "met" because it could not ask is worse than no gate -- it is the
# stack walk that finds no roots and so passes every root check. Every path out
# of a failed fetch here is `exit 2`, and there is no path on which an empty
# answer is read as a good one.
#
# ## The subjects
#
# Discovered, not hardcoded. Two sources, unioned:
#
#   * every issue carrying the label `v1.0-limitation`, whatever its state --
#     so a seventh that joins the six is checked the day it is labelled, with
#     nobody having to remember to add it here;
#   * every issue number named in RELEASE-CRITERIA-1.0.md's criterion 4 -- so
#     that removing the label cannot quietly shrink the set. An issue on that
#     list with the label taken off has to be closed as *fixed*; that is the
#     criterion's other allowed state, and this is where it is read.
#
# A set that can only be shrunk by editing the criteria document is a set whose
# shrinking is a diff somebody reviews.
#
# ## What a documented subject must carry
#
# The five fields LIMITATIONS.md's own "How to read an entry" requires, because
# a section naming an issue and saying nothing is not what the criterion asks
# for. The list is that document's, not this script's: What, Why, Workaround,
# Disposition, Tracked.
#
# Usage: followups-check.sh [tree-root]
#
# Environment:
#   GITHUB_TOKEN / GH_TOKEN  required for the API call, including public
#                            repositories. Missing credentials fail closed.
#   FOLLOWUPS_REPO           owner/name to ask about. Default sinisterMage/WSharp.
#   FOLLOWUPS_FETCH          a command that answers instead of the API, for the
#                            selftest. Called as `$cmd list` and `$cmd get N`,
#                            and expected to write the same TSV `github_fetch`
#                            does, or to exit non-zero.

set -uo pipefail

readonly LABEL="v1.0-limitation"
readonly DOC="LIMITATIONS.md"
readonly CRITERIA="RELEASE-CRITERIA-1.0.md"
readonly CRITERION_HEADING="## 4. "
readonly REPO="${FOLLOWUPS_REPO:-sinisterMage/WSharp}"

# The fields an entry must carry, from LIMITATIONS.md's "How to read an entry".
# Matched as the first word of a bold run at the start of a line, because the
# entries punctuate them differently -- `**What.**`, `**Disposition: documented
# limitation.**`, `**Workaround, and it is the shape a real server has anyway.**`
# -- and holding prose to a punctuation mark is how a check starts costing more
# than it catches.
readonly REQUIRED_FIELDS=(What Why Workaround Disposition Tracked)

fail=0
# Every labelled issue's record, as TSV. Read by `check_subject` so that the
# whole run is one request.
LISTED=""
note() { printf '%s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; fail=1; }
ok() { printf 'ok    %s\n' "$*"; }

# Everything that means "this script could not find out". Never `exit 1`: see
# the header.
cannot() {
  printf 'cannot answer: %s\n' "$*"
  exit 2
}

# --- asking GitHub -----------------------------------------------------------

# One line per issue: number, state, state_reason, labels (comma-separated),
# title. Tab-separated, because a title may hold anything else and `@tsv`
# escapes a tab inside one.
#
# **No field may be empty.** A tab is an IFS *whitespace* character, so `read`
# collapses a run of them into one separator -- an issue with no
# `state_reason`, or with the label removed and no other, would shift every
# field after it by one and be judged on somebody else's data. Hence the two
# placeholders rather than `// ""`.
readonly JQ_ROWS='
  (if type == "array" then .[] else . end)
  | select(.pull_request == null)
  | [ (.number | tostring), .state, (.state_reason // "unset"),
      ([.labels[].name] | join(",") | if . == "" then "-" else . end),
      (.title | if . == "" then "-" else . end) ]
  | @tsv'

api_get() {
  local path="$1" token out body code
  local -a auth=()

  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  [ -n "$token" ] && auth=(-H "Authorization: Bearer $token")

  # The status code is appended on its own line rather than trusted to curl's
  # exit status, because a 403 with a rate-limit body is a successful HTTP
  # transaction and an unsuccessful question.
  out="$(curl -sS -m 30 -w $'\n%{http_code}' \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' \
    ${auth[@]+"${auth[@]}"} \
    "https://api.github.com$path" 2>&1)" || {
    printf 'the request to %s failed: %s\n' "$path" "$out" >&2
    return 1
  }

  code="${out##*$'\n'}"
  body="${out%$'\n'*}"

  case "$code" in
    200) printf '%s' "$body" ;;
    401)
      printf 'GitHub answered 401 for %s: the token was refused.\n' "$path" >&2
      printf 'Supply a valid GITHUB_TOKEN or GH_TOKEN and rerun the gate.\n' >&2
      return 1
      ;;
    403 | 429)
      printf 'GitHub answered %s for %s -- a rate limit or a refused token.\n' "$code" "$path" >&2
      printf 'Set GITHUB_TOKEN; the unauthenticated limit is 60 an hour per address.\n' >&2
      return 1
      ;;
    404)
      printf 'GitHub answered 404 for %s.\n' "$path" >&2
      printf 'Either the repository name is wrong or the token cannot see it.\n' >&2
      return 1
      ;;
    *)
      printf 'GitHub answered %s for %s.\n' "$code" "$path" >&2
      return 1
      ;;
  esac
}

github_fetch() {
  local mode="$1" body rows count
  case "$mode" in
    list)
      body="$(api_get "/repos/$REPO/issues?labels=$LABEL&state=all&per_page=100")" || return 1
      rows="$(printf '%s' "$body" | jq -r "$JQ_ROWS")" || return 1
      # A full page means there may be a second one, and a subject on it would
      # be silently unchecked -- the exact failure this script exists to make
      # loud. Six is the number today; a hundred would be a different project.
      count="$(printf '%s' "$body" | jq -r 'length')" || return 1
      if [ "$count" -ge 100 ]; then
        printf 'the %s label has %s issues, which is a full page.\n' "$LABEL" "$count" >&2
        printf 'This script does not paginate, so it cannot see them all.\n' >&2
        return 1
      fi
      printf '%s\n' "$rows"
      ;;
    get)
      body="$(api_get "/repos/$REPO/issues/$2")" || return 1
      printf '%s' "$body" | jq -r "$JQ_ROWS" || return 1
      ;;
    *)
      printf 'unknown fetch mode %s\n' "$mode" >&2
      return 1
      ;;
  esac
}

fetch() {
  if [ -n "${FOLLOWUPS_FETCH:-}" ]; then
    "$FOLLOWUPS_FETCH" "$@"
    return
  fi
  github_fetch "$@"
}

# --- reading the two documents ----------------------------------------------

# The issue numbers criterion 4 names, one per line. The section is read rather
# than a fixed list kept here, so that the roster lives in the document that
# states the criterion.
criterion_subjects() {
  awk -v heading="$CRITERION_HEADING" '
    index($0, heading) == 1 { inside = 1; next }
    inside && /^## / { inside = 0 }
    inside { print }
  ' "$CRITERIA" | grep -oE '#[0-9]+' | tr -d '#' | sort -un
}

# Every `## ` section of LIMITATIONS.md that names issue N, as the heading text,
# one per line. Both spellings the file uses are accepted: the issue URL, and a
# bare `#N`. The `[^0-9]|$` guard is what keeps `#1` from matching `#15`.
#
# A heading is a run of hashes followed by a space, and only outside a fenced
# code block. Both halves are load-bearing on the real file: the `x86-64 and
# aarch64 only` entry quotes `#[cfg(not(any(...)))]`, which starts with a hash,
# and a looser test cut that section in half above its **Tracked.** line.
sections_naming() {
  local number="$1"
  awk -v n="$number" '
    /^```/ { fence = !fence; next }
    !fence && /^#+ / {
      if (heading != "" && named) print heading
      heading = ($0 ~ /^## /) ? substr($0, 4) : ""
      named = 0
      next
    }
    heading == "" { next }
    {
      if ($0 ~ ("/issues/" n "([^0-9]|$)")) named = 1
      if ($0 ~ ("#" n "([^0-9]|$)")) named = 1
    }
    END { if (heading != "" && named) print heading }
  ' "$DOC"
}

# The fields a named section carries, one per line.
section_fields() {
  local heading="$1"
  awk -v want="$heading" '
    /^```/ { fence = !fence; next }
    fence { next }
    /^#+ / { inside = ($0 ~ /^## /) && (substr($0, 4) == want); next }
    inside && /^\*\*[A-Z][a-z]+[.,:]/ {
      field = $0
      sub(/^\*\*/, "", field)
      sub(/[.,:].*$/, "", field)
      print field
    }
  ' "$DOC"
}

# --- the check itself --------------------------------------------------------

# Holds one subject to one of the criterion's two allowed states.
check_subject() {
  local number="$1" record state reason labels title

  # The list already holds every labelled issue, so `get` is asked only about a
  # roster entry the label no longer covers -- which is the case that needs it
  # and, today, never happens. One request rather than seven matters: the
  # unauthenticated rate limit is sixty an hour for a whole runner address.
  record="$(printf '%s\n' "$LISTED" | grep "^$number	" || true)"
  if [ -z "$record" ]; then
    record="$(fetch get "$number")" || cannot "asking GitHub about issue #$number failed"
    [ -n "$record" ] || cannot "GitHub said nothing about issue #$number"
  fi

  IFS=$'\t' read -r _ state reason labels title <<<"$record"

  if [ "$state" != "closed" ]; then
    bad "#$number is $state: \"$title\""
    note "        Criterion 4 says an issue left open is a failure. Close it as fixed,"
    note "        or close it with the $LABEL label and write its entry in $DOC."
    return
  fi

  # No label means the other allowed state: closed as fixed. Believed only when
  # GitHub says it was completed -- an issue closed as `not_planned` with the
  # label taken off is neither fixed nor documented, and is exactly the quiet
  # third state the criterion forbids.
  if [[ ",$labels," != *",$LABEL,"* ]]; then
    if [ "$reason" = "not_planned" ]; then
      bad "#$number is closed as not planned and carries no $LABEL label: \"$title\""
      note "        That is neither of the two states criterion 4 allows. Either it was"
      note "        fixed -- reopen and close it as completed -- or it is a limitation,"
      note "        in which case put the label back and write its entry in $DOC."
      return
    fi
    [ "$reason" = "completed" ] || cannot "cannot confirm #$number was fixed: closure reason is $reason"
    ok "#$number closed as fixed: \"$title\""
    return
  fi

  # An *entry* is a section naming the issue and carrying at least one of the
  # fields. The qualifier is what tells the front matter's "The six, and what
  # was decided" table -- which names every subject and is an entry for none of
  # them -- from a section somebody wrote and left unfinished. Without it every
  # subject would be "named somewhere" and the two failures would report each
  # other's message.
  local best="" best_missing="" heading field found missing entries=0
  while IFS= read -r heading; do
    [ -n "$heading" ] || continue

    missing=""
    found="$(section_fields "$heading")"
    [ -n "$found" ] || continue
    entries=$((entries + 1))

    for field in "${REQUIRED_FIELDS[@]}"; do
      printf '%s\n' "$found" | grep -qx "$field" || missing="$missing $field"
    done
    if [ -z "$missing" ]; then
      ok "#$number documented in \"$heading\""
      return
    fi
    if [ -z "$best" ] || [ "${#missing}" -lt "${#best_missing}" ]; then
      best="$heading"
      best_missing="$missing"
    fi
  done < <(sections_naming "$number")

  if [ "$entries" -eq 0 ]; then
    bad "#$number is closed as a $LABEL and no entry in $DOC names it: \"$title\""
    note "        A limitation closed on a comment is an undocumented limitation, which"
    note "        is the one thing a 1.0 is not allowed. Add a \`## \` entry to $DOC whose"
    note "        **Tracked.** line links issue $number."
    return
  fi

  bad "#$number is named in \"$best\", which is missing:$best_missing"
  note "        $DOC's own \"How to read an entry\" says an entry missing any of"
  note "        What, Why, Workaround, Disposition or Tracked is not finished, and"
  note "        criterion 4 asks for what does not work, why it is not fixed for 1.0,"
  note "        and what would fix it."
}

main() {
  local root="${1:-.}"

  cd "$root" || cannot "cannot read the tree at $root"

  command -v jq >/dev/null 2>&1 || cannot '`jq` is not on PATH'
  if [ -z "${FOLLOWUPS_FETCH:-}" ]; then
    [ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ] || cannot "GITHUB_TOKEN or GH_TOKEN is required"
    command -v curl >/dev/null 2>&1 || cannot '`curl` is not on PATH'
  fi

  # Criterion 4's evidence is "followups-check.sh exiting 0, plus LIMITATIONS.md
  # on main". The second half is checked here so that deleting the file is a
  # failure rather than a check with nothing to read.
  [ -f "$DOC" ] || cannot "$DOC does not exist, and criterion 4's evidence is that it does"
  [ -f "$CRITERIA" ] || cannot "$CRITERIA does not exist, so the subject roster cannot be read"

  local -a subjects=()
  local n labelled

  LISTED="$(fetch list)" || cannot "asking GitHub for the $LABEL issues failed"

  while IFS=$'\t' read -r n _; do
    [ -n "$n" ] && subjects+=("$n")
  done <<<"$LISTED"
  labelled="${#subjects[@]}"

  while IFS= read -r n; do
    [ -n "$n" ] || continue
    case " ${subjects[*]} " in
      *" $n "*) ;;
      *) subjects+=("$n") ;;
    esac
  done < <(criterion_subjects)

  if [ "${#subjects[@]}" -eq 0 ]; then
    cannot "found no issues labelled $LABEL and none named in $CRITERIA's criterion 4"
  fi

  note "repository     $REPO"
  note "label          $LABEL ($labelled issue(s))"
  note "roster         ${#subjects[@]} subject(s), from the label and $CRITERIA"
  note ""

  # Numeric, so the output reads in issue order whichever source found them.
  local sorted
  sorted="$(printf '%s\n' "${subjects[@]}" | sort -n)"
  while IFS= read -r n; do
    check_subject "$n"
  done <<<"$sorted"

  note ""
  if [ "$fail" -eq 0 ]; then
    note "CRITERION 4 MET: ${#subjects[@]} of ${#subjects[@]} subjects resolved or documented."
    exit 0
  fi
  note "CRITERION 4 NOT MET"
  exit 1
}

main "$@"
