#!/usr/bin/env bash
#
# Answers criterion 3 of RELEASE-CRITERIA-1.0.md: does every defect fixed in the
# window carry a regression guard?
#
# The criterion's rule: "For every issue labelled `defect` closed after the
# freeze-start commit, it asserts that the pull request which closed it touches
# at least one path under that surface's guard directory. A fix with no guard is
# listed by name."
#
# Exit status is the answer, so this composes:
#
#   0  every defect closed in the window was closed by a change carrying a guard
#   1  one was not, and it is named on stdout
#   2  the question cannot be answered -- no `curl`, no `jq`, no network, no
#      token, a rate limit, no window to ask about
#
# **Two is not one, for the reason `followups-check.sh` gives:** this gate has
# to ask GitHub, and a question that cannot be asked must never read as met.
#
# ## Which issues
#
# Closed as completed, in the window, labelled `bug` or `defect`. The intake form
# (`.github/ISSUE_TEMPLATE/defect.yml`) applies `bug`; the criterion says
# `defect`; both are read, so that neither spelling is the one that slips
# through. An issue closed as *not planned* was not fixed and has nothing to
# guard, and one labelled `v1.0-limitation` is criterion 4's, not this one's.
#
# ## Which change closed it
#
# The commit on the issue's last `closed` event, and the merged pull request that
# commit belongs to -- which is what "the pull request which closed it" is when a
# `Fixes #N` merges. An issue closed by hand has no such commit, so the merged
# pull requests that cross-referenced it are read instead, and every one of them
# counts: the fix is somewhere in them. An issue closed by hand that no merged
# pull request mentions is unguarded by definition -- nothing can be shown to
# have fixed it.
#
# ## What counts as a guard
#
# Criterion 3's table, by the issue's `area:` label:
#
#   area: syntax, sema, codegen, stdlib, runtime, start
#       tests/cases/, a crate's tests/ directory, or a `#[test]` the change adds
#       beside the code -- "runtime internals with no W#-visible surface" are
#       guarded by a unit test, and a path cannot say whether a change to
#       `heap.rs` added one, so the patch is read for an added `#[test]`
#   area: cli
#       crates/wsharp-cli/tests/ or tests/cases/
#   no area label
#       any of the above, or a CI job (.github/workflows/) or a script's own
#       test (scripts/tests/, tests/harness/ selftests) -- the rows for "the
#       build and test commands themselves" and for the release pipeline
#
# In sinisterMage/sharpie the table has one row: tests/.
#
# Usage: guard-check.sh [--since YYYY-MM-DD]
#
# The window starts at the freeze-start commit recorded in `.github/freeze-start`
# (see `freeze-check.sh`). Before Day 0 is declared there is no window, and the
# answer is 2 -- unless `--since` names one, which is how the gate is run ahead
# of the freeze to find out what the window would contain.
#
# Environment:
#   GITHUB_TOKEN / GH_TOKEN  required. Missing credentials fail closed.
#   GUARD_REPO               owner/name to ask about. Default sinisterMage/WSharp.
#   GUARD_FETCH              a command that answers instead of the API, for the
#                            selftest. Called as `$cmd issues SINCE` and
#                            `$cmd changes N`; see `github_fetch` for what each
#                            writes. Non-zero means "could not ask".

set -uo pipefail

readonly REPO="${GUARD_REPO:-sinisterMage/WSharp}"
readonly START_FILE="${FREEZE_START_FILE:-.github/freeze-start}"

fail=0
note() { printf '%s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; fail=1; }
ok() { printf 'ok    %s\n' "$*"; }
cannot() {
  printf 'cannot answer: %s\n' "$*"
  exit 2
}

since=""
while [ $# -gt 0 ]; do
  case "$1" in
    --since) since="${2:-}"; shift 2 ;;
    -h | --help) sed -n '2,/^set -uo/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d'; exit 0 ;;
    *) printf 'guard-check: unknown argument %s\n' "$1" >&2; exit 2 ;;
  esac
done

if [ -z "$since" ]; then
  [ -f "$START_FILE" ] || cannot "no freeze start declared in $START_FILE; pass --since YYYY-MM-DD to check a window anyway"
  start_sha="$(awk 'NF { print $1; exit }' "$START_FILE")"
  [ -n "$start_sha" ] || cannot "$START_FILE names no commit"
  since="$(git log -1 --format=%cs "$start_sha" 2>/dev/null)" ||
    cannot "the freeze-start commit $start_sha is not in this clone (fetch with full history)"
fi
case "$since" in
  [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;;
  *) cannot "--since wants YYYY-MM-DD, not '$since'" ;;
esac

# --- asking GitHub -----------------------------------------------------------

api_get() {
  local path="$1" token out body code
  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  [ -n "$token" ] || { printf 'no GITHUB_TOKEN or GH_TOKEN, so nothing can be asked\n' >&2; return 1; }
  # The status code on a line of its own: a rate-limited 403 is a successful
  # HTTP transaction and an unsuccessful question.
  out="$(curl -sS -m 30 -w $'\n%{http_code}' \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' \
    -H "Authorization: Bearer $token" \
    "https://api.github.com$path" 2>&1)" || {
    printf 'the request to %s failed: %s\n' "$path" "$out" >&2
    return 1
  }
  code="${out##*$'\n'}"
  body="${out%$'\n'*}"
  [ "$code" = 200 ] || { printf 'GitHub answered %s for %s\n' "$code" "$path" >&2; return 1; }
  printf '%s' "$body"
}

# A page of 100 that comes back full may have a second page, and an issue on it
# would be silently unchecked. Refuse rather than paginate: a window with a
# hundred defect fixes in it is not a freeze.
full_page() {
  local n
  n="$(printf '%s' "$1" | jq -r 'length')" || return 0
  [ "$n" -ge 100 ]
}

# issues SINCE: one line per issue -- number, closed date, state_reason, labels
# (comma-separated, `-` for none). No field is empty, because `read` collapses a
# run of tabs.
# changes N: one line per path the closing change touched -- path, then `test`
# if the patch adds a `#[test]` and `-` otherwise -- preceded by one line naming
# the change (`pr 123`, `commit abc...`), or the single line `none` when nothing
# can be shown to have closed it.
github_fetch() {
  local mode="$1" body label seen="" rows=""
  case "$mode" in
    issues)
      for label in bug defect; do
        body="$(api_get "/repos/$REPO/issues?state=closed&labels=$label&since=${2}T00:00:00Z&per_page=100")" || return 1
        full_page "$body" && { printf 'the %s label has a full page of closed issues since %s\n' "$label" "$2" >&2; return 1; }
        rows+="$(printf '%s' "$body" | jq -r --arg since "$2" '
          .[] | select(.pull_request == null)
              | select((.closed_at // "") >= $since)
              | [ (.number | tostring), (.closed_at[0:10]), (.state_reason // "unset"),
                  ([.labels[].name] | join(",") | if . == "" then "-" else . end) ]
              | @tsv')"$'\n' || return 1
      done
      # Labelled both ways is one issue.
      printf '%s' "$rows" | awk -F'\t' 'NF && !seen[$1]++'
      ;;
    changes)
      local n="$2" timeline sha prs pr files
      timeline="$(api_get "/repos/$REPO/issues/$n/timeline?per_page=100")" || return 1
      full_page "$timeline" && { printf 'issue #%s has a full page of timeline\n' "$n" >&2; return 1; }
      sha="$(printf '%s' "$timeline" | jq -r '[.[] | select(.event == "closed")] | last | .commit_id // empty')" || return 1
      if [ -n "$sha" ]; then
        prs="$(api_get "/repos/$REPO/commits/$sha/pulls")" || return 1
        pr="$(printf '%s' "$prs" | jq -r --arg sha "$sha" '
          [.[] | select(.merged_at != null)]
          | (map(select(.merge_commit_sha == $sha)) + .) | first | .number // empty')" || return 1
      else
        # Closed by hand: every merged pull request that mentions it.
        pr="$(printf '%s' "$timeline" | jq -r '
          [.[] | select(.event == "cross-referenced")
               | .source.issue | select(.pull_request.merged_at != null) | .number]
          | unique | map(tostring) | join(" ")')" || return 1
      fi
      if [ -n "$pr" ]; then
        for n in $pr; do
          printf 'pr %s\n' "$n"
          files="$(api_get "/repos/$REPO/pulls/$n/files?per_page=100")" || return 1
          full_page "$files" && { printf 'pull request #%s touches a full page of files\n' "$n" >&2; return 1; }
          printf '%s' "$files" | jq -r '.[] | [ .filename,
            (if ((.patch // "") | test("(^|\n)\\+\\s*#\\[test\\]")) then "test" else "-" end) ] | @tsv' || return 1
        done
      elif [ -n "$sha" ]; then
        printf 'commit %s\n' "$sha"
        files="$(api_get "/repos/$REPO/commits/$sha")" || return 1
        printf '%s' "$files" | jq -r '.files[] | [ .filename,
          (if ((.patch // "") | test("(^|\n)\\+\\s*#\\[test\\]")) then "test" else "-" end) ] | @tsv' || return 1
      else
        printf 'none\n'
      fi
      ;;
    *) printf 'unknown fetch mode %s\n' "$mode" >&2; return 1 ;;
  esac
}

fetch() {
  if [ -n "${GUARD_FETCH:-}" ]; then
    "$GUARD_FETCH" "$@"
    return
  fi
  github_fetch "$@"
}

if [ -z "${GUARD_FETCH:-}" ]; then
  command -v curl >/dev/null || cannot "curl is not installed"
  command -v jq >/dev/null || cannot "jq is not installed"
fi

# --- what a guard is ---------------------------------------------------------

# is_guard AREA PATH TEST: whether one touched path guards a defect in AREA.
is_guard() {
  local area="$1" path="$2" test="$3"
  if [[ "$REPO" == */sharpie ]]; then
    [[ "$path" == tests/* ]]
    return
  fi
  case "$area" in
    syntax | sema | codegen | stdlib | runtime | start)
      [[ "$path" == tests/cases/* || "$path" =~ ^crates/[^/]+/tests/ ]] && return 0
      [[ "$test" == test && "$path" =~ ^crates/[^/]+/src/ ]] && return 0
      return 1
      ;;
    cli)
      [[ "$path" == crates/wsharp-cli/tests/* || "$path" == tests/cases/* ]]
      return
      ;;
    *)
      [[ "$path" == tests/cases/* || "$path" =~ ^crates/[^/]+/tests/ ]] && return 0
      [[ "$test" == test && "$path" =~ ^crates/[^/]+/src/ ]] && return 0
      [[ "$path" == .github/workflows/* || "$path" == scripts/tests/* ]] && return 0
      [[ "$path" =~ ^tests/(harness|conformance)/ && "$path" =~ (selftest|\.test\.sh$) ]] && return 0
      return 1
      ;;
  esac
}

# --- the window --------------------------------------------------------------

note "repository     $REPO"
note "window         closed on or after $since"

issues="$(fetch issues "$since")" || cannot "could not list the closed defects"

checked=0
while IFS=$'\t' read -r number closed reason labels; do
  [ -n "$number" ] || continue
  if [ "$reason" = not_planned ]; then
    note "skip  #$number closed as not planned"
    continue
  fi
  if [[ ",$labels," == *",v1.0-limitation,"* ]]; then
    note "skip  #$number is a documented limitation (criterion 4)"
    continue
  fi
  area="$(printf '%s' "$labels" | tr ',' '\n' | sed -n 's/^area: //p' | head -1)"
  changes="$(fetch changes "$number")" || cannot "could not read what closed #$number"
  checked=$((checked + 1))

  if [ "$(printf '%s\n' "$changes" | head -1)" = none ]; then
    bad "#$number (closed $closed) was closed by no pull request or commit, so nothing guards it"
    continue
  fi
  closer="$(printf '%s\n' "$changes" | grep -E '^(pr|commit) ' | sed 's/^pr /#/; s/^commit //' | paste -sd' ' -)"
  guard=""
  while IFS=$'\t' read -r path test; do
    [ -n "$path" ] || continue
    case "$path" in pr\ * | commit\ *) continue ;; esac
    if is_guard "$area" "$path" "$test"; then
      guard="$path"
      break
    fi
  done <<<"$changes"
  if [ -n "$guard" ]; then
    ok "#$number guarded by $closer: $guard"
  else
    bad "#$number (closed $closed${area:+, area: $area}) fixed by $closer, which touches no guard"
  fi
done <<<"$issues"

note ""
if [ "$fail" -eq 0 ]; then
  note "CRITERION 3 MET: $checked defect fix(es) in the window, every one guarded"
else
  note "CRITERION 3 NOT MET: the fixes above carry no regression guard"
fi
exit "$fail"
