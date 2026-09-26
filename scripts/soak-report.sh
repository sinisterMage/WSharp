#!/usr/bin/env sh
# Answers criterion 1 of RELEASE-CRITERIA-1.0.md: did every soak subject report,
# every day of the window, and did every report pass?
#
# Two questions, and the first is the one that is easy to lose:
#
#   1. Is there a row for every subject for every day in the window?
#   2. Does every row say the run passed?
#
# A missing row is a failure, not a gap. A soak that stopped reporting is a soak
# that stopped, and a report that quietly averaged over the days it had would
# turn three weeks of silence into a number that looks like evidence. So absence
# is counted, named by date, and fails the gate exactly as a bad row does.
#
# ## The row format
#
# Each subject's owner appends its own rows; this script only reads them. One
# file per subject under `$SOAK_DIR`, named `<subject>.tsv`, six tab-separated
# fields and no header:
#
#     2026-09-26<TAB>compiler<TAB>ok<TAB><40-char sha><TAB>x86_64-unknown-linux-gnu<TAB>257 cases, 0 diverged
#     date        subject      verdict  commit            platform                   detail
#
#   date      UTC, `YYYY-MM-DD`, the day the run *covered*
#   subject   must match the file's name, so a row appended to the wrong file
#             is caught rather than silently credited to the wrong owner
#   verdict   `ok` or `fail`; anything else is a malformed row and fails
#   commit    the full SHA the subject was exercised at
#   platform  a target triple, or `-` where the subject is not per-platform
#   detail    free text, printed beside a failure
#
# Append one with `--append`, which is the only writer, so the format has one
# definition:
#
#     scripts/soak-report.sh --append --subject compiler --verdict ok \
#         --commit "$(git rev-parse HEAD)" \
#         --platform x86_64-unknown-linux-gnu --detail '257 cases, 0 diverged'
#
# Two rows for one subject on one day are not an error -- a subject run on four
# triples writes four. The day counts as reported when at least one row exists,
# and fails when *any* row for it failed, because a soak that passed on three
# platforms and failed on the fourth did not pass.
#
# ## Subjects
#
# `$SOAK_DIR/subjects.tsv`, so adding a subject is a diff somebody reviews
# rather than a change to this script:
#
#     compiler<TAB>Dex<TAB>tests/harness/nightly.sh, once a day
#
# ## Exit status
#
#   0  every subject reported every day, and every row passed
#   1  a day is missing or a row failed; the reasons are on stdout
#   2  the question cannot be answered -- no subjects manifest, or a `date`
#      that can do neither GNU nor BSD arithmetic
#
# One and two are deliberately different, exactly as in `freeze-check.sh`:
# "not met" is a fact about the project, "cannot answer" is a fact about this
# script's inputs, and a checklist that confused them would tick a box it never
# checked.
#
# POSIX sh: this runs on the Linux, macOS and Git Bash runners alike.

set -eu

WINDOW="${SOAK_WINDOW_DAYS:-28}"
SOAK_DIR="${SOAK_DIR:-soak}"
END=""
MODE=report

# --append fields
A_SUBJECT=""
A_VERDICT=""
A_COMMIT=""
A_PLATFORM="-"
A_DETAIL="-"
A_DATE=""

while [ $# -gt 0 ]; do
	case "$1" in
	--window) WINDOW="$2"; shift 2 ;;
	--dir) SOAK_DIR="$2"; shift 2 ;;
	--end) END="$2"; shift 2 ;;
	--append) MODE=append; shift ;;
	--subject) A_SUBJECT="$2"; shift 2 ;;
	--verdict) A_VERDICT="$2"; shift 2 ;;
	--commit) A_COMMIT="$2"; shift 2 ;;
	--platform) A_PLATFORM="$2"; shift 2 ;;
	--detail) A_DETAIL="$2"; shift 2 ;;
	--date) A_DATE="$2"; shift 2 ;;
	-h | --help) sed -n '2,72p' "$0"; exit 0 ;;
	*) echo "soak-report: unknown argument $1" >&2; exit 2 ;;
	esac
done

note() { printf '%s\n' "$*"; }
ok() { printf 'ok    %s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; }

# GNU date and BSD date disagree about everything except the output format.
day_before() {
	date -u -d "$1 -$2 days" +%Y-%m-%d 2>/dev/null ||
		date -u -j -v-"$2"d -f %Y-%m-%d "$1" +%Y-%m-%d
}

# --- appending ---------------------------------------------------------------

if [ "$MODE" = append ]; then
	[ -n "$A_SUBJECT" ] || { echo "soak-report: --append needs --subject" >&2; exit 2; }
	case "$A_VERDICT" in
	ok | fail) ;;
	*) echo "soak-report: --verdict must be 'ok' or 'fail'" >&2; exit 2 ;;
	esac
	[ -n "$A_COMMIT" ] || { echo "soak-report: --append needs --commit" >&2; exit 2; }
	[ -n "$A_DATE" ] || A_DATE="$(date -u +%Y-%m-%d)"
	mkdir -p "$SOAK_DIR"
	# Tabs are the separator, so a tab inside a field would make a row that
	# reads as a different row. Detail is the only free-text field; flatten it.
	A_DETAIL="$(printf '%s' "$A_DETAIL" | tr '\t\n' '  ')"
	printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
		"$A_DATE" "$A_SUBJECT" "$A_VERDICT" "$A_COMMIT" "$A_PLATFORM" "$A_DETAIL" \
		>>"$SOAK_DIR/$A_SUBJECT.tsv"
	exit 0
fi

# --- reporting ---------------------------------------------------------------

SUBJECTS="$SOAK_DIR/subjects.tsv"
[ -f "$SUBJECTS" ] || {
	note "cannot answer: no subject manifest at $SUBJECTS"
	note ""
	note "One line per soak subject -- name, owner, what it is:"
	note ""
	note "    compiler<TAB>Dex<TAB>tests/harness/nightly.sh, once a day"
	note ""
	note "See RELEASE-CRITERIA-1.0.md, criterion 1."
	exit 2
}

[ -n "$END" ] || END="$(date -u +%Y-%m-%d)"
day_before "$END" 0 >/dev/null 2>&1 || {
	note "cannot answer: this \`date\` understands neither -d nor -j"
	exit 2
}

# The window, newest day first, as a space-separated list.
days=""
i=0
while [ "$i" -lt "$WINDOW" ]; do
	days="$days $(day_before "$END" "$i")"
	i=$((i + 1))
done

note "soak window    $WINDOW days ending $END (UTC)"
note "rows           $SOAK_DIR/<subject>.tsv"
note ""

fail=0
subject_count=0

# `printf '| %s ' ...` per day would be a 28-column table nobody can read in a
# terminal. One line per subject, one character per day, oldest on the left:
#
#     .  reported and passed        x  reported and failed        ?  no row
note "Calendar, oldest day first:"
note ""

while IFS='	' read -r subject owner what; do
	case "$subject" in
	'' | '#'*) continue ;;
	esac
	subject_count=$((subject_count + 1))
	file="$SOAK_DIR/$subject.tsv"

	calendar=""
	missing=""
	failed=""
	# Oldest first, so the calendar reads left to right like a calendar.
	i="$WINDOW"
	while [ "$i" -gt 0 ]; do
		i=$((i - 1))
		day="$(day_before "$END" "$i")"
		if [ ! -f "$file" ]; then
			calendar="$calendar?"
			missing="$missing $day"
			continue
		fi
		# Rows for this day for this subject. The subject field is checked
		# against the file's name so a row appended to the wrong file is a
		# malformed row rather than credit for a day nobody ran.
		verdicts="$(awk -F'\t' -v d="$day" -v s="$subject" \
			'$1==d && $2==s {print $3}' "$file")"
		if [ -z "$verdicts" ]; then
			calendar="$calendar?"
			missing="$missing $day"
		elif printf '%s\n' "$verdicts" | grep -qvx 'ok'; then
			calendar="${calendar}x"
			failed="$failed $day"
		else
			calendar="$calendar."
		fi
	done

	printf '  %-14s %s\n' "$subject" "$calendar"

	if [ -n "$missing" ] || [ -n "$failed" ]; then
		fail=1
		[ -n "$missing" ] && printf '    %-12s %s\n' "no row:" "${missing# }"
		if [ -n "$failed" ]; then
			printf '    %-12s %s\n' "failed:" "${failed# }"
			for day in $failed; do
				awk -F'\t' -v d="$day" -v s="$subject" \
					'$1==d && $2==s && $3!="ok" {printf("      %s  %s  %s  %s\n", $1, $5, $3, $6)}' \
					"$file"
			done
		fi
		printf '    %-12s %s\n' "owner:" "$owner${what:+ — $what}"
	fi
done <"$SUBJECTS"

note ""
note "  .  reported and passed    x  reported and failed    ?  no row"
note ""

if [ "$subject_count" -eq 0 ]; then
	note "cannot answer: $SUBJECTS names no subjects"
	exit 2
fi

if [ "$fail" -eq 0 ]; then
	ok "$subject_count subject(s) reported every day of the $WINDOW-day window, all passing"
	exit 0
fi
bad "the soak window is not complete"
exit 1
