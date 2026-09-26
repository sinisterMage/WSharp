#!/usr/bin/env sh
# Answers criterion 2 of RELEASE-CRITERIA-1.0.md: is the stability freeze met?
#
# Three questions, each answered by a query rather than by anybody's memory:
#
#   1. Have at least WINDOW days passed since the declared freeze-start commit?
#   2. Did any pull request merged into `main` during that window carry the
#      label `change:breaking`?
#   3. Is any issue labelled `P1` open in either repository?
#
# Day 0 is declared by writing `.github/freeze-start`, one line:
#
#     <full-commit-sha> <YYYY-MM-DD>
#
# A file rather than a tag or a date in prose, because the clock reset rule
# means this date moves, and a file that moves is a diff somebody can review.
#
# Exit status is the answer, so this composes:
#
#   0  the freeze is met
#   1  the freeze is not met, and the reasons are on stdout
#   2  the question cannot be answered -- day 0 not declared, or no `gh`
#
# Two and one are deliberately different. "Not met" is a fact about the project;
# "cannot answer" is a fact about this script's inputs, and a release checklist
# that treated the second as the first would tick a box it never checked.
#
# POSIX sh: this runs on the Linux, macOS and Git Bash runners alike, and the
# only thing it assumes past that is `gh` and a `date` that can do arithmetic
# on an ISO date -- which GNU and BSD spell differently, so both are tried.

set -eu

WINDOW="${FREEZE_WINDOW_DAYS:-28}"
START_FILE="${FREEZE_START_FILE:-.github/freeze-start}"
REPOS="${FREEZE_REPOS:-sinisterMage/WSharp sinisterMage/sharpie}"

fail=0
note() { printf '%s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; fail=1; }
ok() { printf 'ok    %s\n' "$*"; }

command -v gh >/dev/null 2>&1 || {
	note "cannot answer: \`gh\` is not on PATH"
	exit 2
}

[ -f "$START_FILE" ] || {
	note "cannot answer: no freeze start declared"
	note ""
	note "Write $START_FILE with one line -- the day 0 commit and its date:"
	note ""
	note "    \$(git rev-parse HEAD) $(date -u +%Y-%m-%d)"
	note ""
	note "Only Johnny declares day 0. See RELEASE-CRITERIA-1.0.md, \"The freeze clock\"."
	exit 2
}

start_sha=$(awk 'NR==1{print $1}' "$START_FILE")
start_date=$(awk 'NR==1{print $2}' "$START_FILE")

case "$start_date" in
[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;;
*)
	note "cannot answer: $START_FILE line 1 is not '<sha> <YYYY-MM-DD>'"
	exit 2
	;;
esac

# GNU date and BSD date disagree about everything except the output format.
epoch_of() {
	date -u -d "$1" +%s 2>/dev/null || date -u -j -f %Y-%m-%d "$1" +%s
}

start_epoch=$(epoch_of "$start_date") || {
	note "cannot answer: this \`date\` understands neither -d nor -j"
	exit 2
}
now_epoch=$(date -u +%s)
days=$(((now_epoch - start_epoch) / 86400))

note "freeze start   $start_sha  $start_date"
note "window         $WINDOW days"
note "elapsed        $days days"
note ""

# --- 1. long enough ----------------------------------------------------------

if [ "$days" -ge "$WINDOW" ]; then
	ok "the window is complete ($days >= $WINDOW days)"
else
	bad "the window is not complete: $days of $WINDOW days"
fi

# --- 2. no breaking change in the window -------------------------------------
#
# The `change:*` label is what makes this a query. A pull request carrying none
# is not evidence of no breaking change -- it is an unanswered question -- so an
# unlabelled merge inside the window fails this gate just as a breaking one
# does.

if ! breaking=$(gh pr list --repo sinisterMage/WSharp --state merged --base main --limit 200 \
	--search "merged:>=$start_date label:change:breaking" \
	--json number,title --jq '.[] | "#\(.number) \(.title)"'); then
	note "cannot answer: querying merged pull requests failed"
	exit 2
fi

if [ -n "$breaking" ]; then
	bad "a breaking change merged inside the window:"
	printf '        %s\n' "$breaking"
else
	ok "no merge labelled change:breaking inside the window"
fi

if ! unlabelled=$(gh pr list --repo sinisterMage/WSharp --state merged --base main --limit 200 \
	--search "merged:>=$start_date -label:change:breaking -label:change:additive -label:change:fix -label:change:none" \
	--json number,title --jq '.[] | "#\(.number) \(.title)"'); then
	note "cannot answer: querying merged pull requests failed"
	exit 2
fi

if [ -n "$unlabelled" ]; then
	bad "a merge inside the window carries no change: label, so it cannot be judged:"
	printf '        %s\n' "$unlabelled"
else
	ok "every merge inside the window carries a change: label"
fi

# How many merges the two gates above actually looked at. A window with no
# merges in it passes both of them for the wrong reason, and a checklist tick
# that cannot tell "nothing broke" from "nothing was examined" is the same
# mistake as a root walk that finds no roots and reports success.
if ! merged=$(gh pr list --repo sinisterMage/WSharp --state merged --base main --limit 200 \
	--search "merged:>=$start_date" --json number --jq 'length'); then
	note "cannot answer: counting merged pull requests failed"
	exit 2
fi
if [ "$merged" -eq 0 ]; then
	note "      (examined 0 merges -- both label gates above are vacuous)"
else
	note "      (examined $merged merges)"
fi

# --- 3. no open P1 -----------------------------------------------------------
#
# Both repositories, because criterion 7 makes sharpie part of the release and a
# P1 there is a P1 against the tag.
#
# The filtering is `--jq` over the labels rather than `--label P1`, because `gh`
# refuses a label the repository has never used -- and a gate script that
# swallowed that would read "no open P1" out of an error. Selecting from the
# returned issues gives the same answer and cannot fail that way.

for repo in $REPOS; do
	if ! p1=$(gh issue list --repo "$repo" --state open --limit 200 \
		--json number,title,labels \
		--jq '.[] | select(any(.labels[]; .name == "P1")) | "#\(.number) \(.title)"'); then
		note "cannot answer: querying open issues in $repo failed"
		exit 2
	fi
	if [ -n "$p1" ]; then
		bad "$repo has an open P1:"
		printf '        %s\n' "$p1"
	else
		ok "$repo has no open P1"
	fi
done

note ""
if [ "$fail" -eq 0 ]; then
	note "FREEZE MET as of $(date -u +%Y-%m-%dT%H:%M:%SZ)"
	exit 0
fi
note "FREEZE NOT MET"
exit 1
