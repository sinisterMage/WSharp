#!/usr/bin/env sh
# Does `soak-report.sh` answer correctly?
#
# It is criterion 1's gate, and the thing it is most important to get right is
# the one that is easiest to get wrong in the safe-looking direction: a missing
# row must fail. A report that treated absence as "nothing to say" would pass a
# window in which a subject stopped running on day three, which is exactly the
# outcome the criterion was written to prevent.
#
# So this builds windows whose answers are known -- complete, one day missing,
# one row failed, a row filed under the wrong subject -- and asserts both the
# exit status and what was printed.
#
# Needs nothing but a POSIX shell and a `date` that can do arithmetic.
#
# Usage:
#   scripts/soak-report-selftest.sh

set -u

HERE=$(cd "$(dirname "$0")" && pwd)
REPORT="$HERE/soak-report.sh"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/soak-selftest-XXXXXX")
trap 'rm -rf "$WORK"' EXIT

failures=0
ok() { printf 'ok    %s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; failures=$((failures + 1)); }

# A fixed end date, so the test says the same thing on every day it is run.
END=2026-09-26

fresh() {
	rm -rf "$WORK/d"
	mkdir -p "$WORK/d"
	printf 'compiler\tDex\tthe nightly\n' >"$WORK/d/subjects.tsv"
}

append() {
	sh "$REPORT" --dir "$WORK/d" --append --subject "$1" --verdict "$2" \
		--commit 0000000000000000000000000000000000000000 \
		--platform x86_64-unknown-linux-gnu --detail "$3" --date "$4"
}

report() {
	sh "$REPORT" --dir "$WORK/d" --window 5 --end "$END" >"$WORK/out" 2>&1
	printf '%s' $?
}

days_ok() {
	for d in 22 23 24 25 26; do append compiler ok "clean" "2026-09-$d"; done
}

# --- a complete window passes ------------------------------------------------
fresh
days_ok
if [ "$(report)" = 0 ]; then
	ok "a complete window exits 0"
else
	bad "a complete window did not exit 0"
	cat "$WORK/out"
fi

# --- a missing day fails, and is named ---------------------------------------
#
# The load-bearing assertion. Absence is a failure, not a gap.
fresh
for d in 22 23 25 26; do append compiler ok "clean" "2026-09-$d"; done
status=$(report)
if [ "$status" = 1 ]; then
	ok "a missing day exits 1"
else
	bad "a missing day exited $status, wanted 1"
fi
if grep -q '2026-09-24' "$WORK/out"; then
	ok "the missing day is named"
else
	bad "the missing day was not named in the output"
	cat "$WORK/out"
fi

# --- a subject with no file at all fails every day ---------------------------
fresh
status=$(report)
if [ "$status" = 1 ]; then
	ok "a subject that never reported exits 1"
else
	bad "a subject that never reported exited $status, wanted 1"
fi
if grep -q '?????' "$WORK/out"; then
	ok "a subject that never reported is five question marks, not an empty row"
else
	bad "a subject with no rows did not render as missing days"
	cat "$WORK/out"
fi

# --- a failed row fails, and its detail is printed ---------------------------
fresh
days_ok
append compiler fail "the collector aborted under stress" "2026-09-25"
status=$(report)
if [ "$status" = 1 ]; then
	ok "a failed row exits 1"
else
	bad "a failed row exited $status, wanted 1"
fi
if grep -q 'the collector aborted under stress' "$WORK/out"; then
	ok "the failing row's detail is printed"
else
	bad "the failing row's detail was not printed"
	cat "$WORK/out"
fi

# --- one platform failing is the day failing ---------------------------------
#
# Four triples write four rows. Three passing does not make the day a pass.
fresh
days_ok
append compiler fail "aarch64-apple-darwin: 2 diverged" "2026-09-26"
if [ "$(report)" = 1 ]; then
	ok "a day with one failed row among several fails"
else
	bad "a day passed despite one of its rows failing"
fi

# --- a row filed under the wrong subject is not credited ---------------------
fresh
for d in 22 23 25 26; do append compiler ok "clean" "2026-09-$d"; done
# The right date, the wrong subject name, in the right file.
printf '2026-09-24\tecosystem\tok\tsha\t-\tmislaid\n' >>"$WORK/d/compiler.tsv"
if [ "$(report)" = 1 ]; then
	ok "a row naming another subject does not fill the gap"
else
	bad "a row naming another subject was credited to this one"
	cat "$WORK/out"
fi

# --- no manifest is 'cannot answer', not 'not met' ---------------------------
rm -rf "$WORK/e"
mkdir -p "$WORK/e"
sh "$REPORT" --dir "$WORK/e" --window 5 --end "$END" >"$WORK/out" 2>&1
status=$?
if [ "$status" = 2 ]; then
	ok "no subject manifest exits 2, not 1"
else
	bad "no subject manifest exited $status, wanted 2"
fi

# --- a malformed verdict is refused at the writer ----------------------------
fresh
sh "$REPORT" --dir "$WORK/d" --append --subject compiler --verdict maybe \
	--commit x >/dev/null 2>&1
if [ $? = 2 ]; then
	ok "a verdict that is neither ok nor fail is refused"
else
	bad "a malformed verdict was written"
fi

# --- the repository's own manifest parses ------------------------------------
#
# Not a judgement about whether the soak is passing -- it is not, and should not
# be, before the window starts. Only that the manifest is readable and names
# its subjects, so the gate fails for a reason about the project.
if sh "$REPORT" --window 1 >"$WORK/out" 2>&1 || [ $? = 1 ]; then
	if grep -q 'compiler' "$WORK/out" && grep -q 'raython' "$WORK/out"; then
		ok "soak/subjects.tsv parses and names its subjects"
	else
		bad "soak/subjects.tsv did not render its subjects"
		cat "$WORK/out"
	fi
else
	bad "soak/subjects.tsv could not be read"
	cat "$WORK/out"
fi

echo
if [ "$failures" -eq 0 ]; then
	echo "soak-report selftest: the gate answers correctly"
	exit 0
fi
echo "soak-report selftest: $failures assertion(s) failed" >&2
exit 1
