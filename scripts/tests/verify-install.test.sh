#!/usr/bin/env bash
#
# Tests for scripts/verify-install.sh.
#
#   bash scripts/tests/verify-install.test.sh
#
# Criterion 7's verifier is the thing that says an install is clean, so a bug
# in it is a wrong answer about the release rather than a wrong answer about a
# program. It gets its own tests for the same reason the change-label rule
# does: a gate whose logic is unverified can pass everything and be believed.
#
# These drive the real script against a real published release -- there is no
# way to test "did the download verify before extraction" against a stub -- and
# use `--tool sharpie`, whose tarball is about a megabyte and needs no C
# compiler, rather than the compiler's seven.
#
# Network is required, which is true of the script and of the job that runs it.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
readonly ROOT
readonly SCRIPT="$ROOT/scripts/verify-install.sh"
readonly VERSION="0.1.2"
readonly TRIPLE="x86_64-unknown-linux-gnu"

checks=0
failures=0

pass() { checks=$((checks + 1)); echo "ok   $*"; }
fail() {
  checks=$((checks + 1))
  failures=$((failures + 1))
  echo "FAIL $*"
}

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/verify-install-test.XXXXXX")" || exit 3
trap 'rm -rf "$SANDBOX"' EXIT

# ---------------------------------------------------------------------------
# A triple the script cannot name is refused. The criterion forbids claiming a
# platform by inference from another platform's run, and this is the mechanism.
# ---------------------------------------------------------------------------

out="$(bash "$SCRIPT" "$VERSION" x86_64-unknown-freebsd 2>&1)"
status=$?
if [ "$status" -eq 2 ] && printf '%s' "$out" | grep -q "not one of the four release triples"; then
  pass "an unknown triple is refused with a message"
else
  fail "an unknown triple is refused with a message: exit $status"
  printf '%s\n' "$out" | head -5 | sed 's/^/     /'
fi

# ---------------------------------------------------------------------------
# The install is clean when TMPDIR is an ordinary directory outside HOME.
# ---------------------------------------------------------------------------

run_in() {
  # usage: run_in <home> <tmpdir>
  env HOME="$1" TMPDIR="$2" bash "$SCRIPT" "$VERSION" "$TRIPLE" --tool sharpie 2>&1
}

plain_home="$SANDBOX/plain/home"
plain_tmp="$SANDBOX/plain/tmp"
mkdir -p "$plain_home" "$plain_tmp"

out="$(run_in "$plain_home" "$plain_tmp")"
status=$?
if [ "$status" -eq 0 ]; then
  pass "a published sharpie release verifies clean (exit 0)"
else
  fail "a published sharpie release verifies clean: exit $status"
  printf '%s\n' "$out" | grep -E '^(FAIL|     )' | head -12 | sed 's/^/     /'
fi

# ---------------------------------------------------------------------------
# The regression this file was written for.
#
# WORK must be pruned from the filesystem manifest under whatever spelling the
# walk uses, not only the one `mktemp` was given. Windows is where this bites
# in production: Git bash mounts `/tmp` onto a directory under `$HOME`, so WORK
# is `/tmp/verify-install.XXXX` to us and `/c/Users/.../Temp/verify-install.XXXX`
# to a walk of `$HOME`, the prune matches nothing, and the check reports the
# extracted prefix as a path created outside the prefix.
#
# A symlinked TMPDIR component reproduces exactly that on any platform: `find`
# does not follow symlinks, so it reaches WORK by the real path while the
# script holds the aliased one.
# ---------------------------------------------------------------------------

aliased="$SANDBOX/aliased"
mkdir -p "$aliased/home/real/tmp"
ln -s real "$aliased/home/link" 2>/dev/null

# Git bash on Windows copies rather than symlinks unless it is told otherwise,
# and a copy is not an alias -- WORK would then be found under the very path
# the script holds, and this case would pass without proving anything. Say so
# rather than asserting on it: Windows's own version of this bug is the `/tmp`
# mount, and the criterion 7 job on that platform is what covers it.
if [ ! -L "$aliased/home/link" ]; then
  echo "skip WORK-prune-by-alias: this shell does not make real symlinks"
  echo "     (the Windows equivalent is the /tmp mount, covered by the"
  echo "      criterion 7 job on windows-latest)"
else
  out="$(run_in "$aliased/home" "$aliased/home/link/tmp")"
  status=$?
  if [ "$status" -eq 0 ]; then
    pass "WORK is pruned when TMPDIR reaches HOME by another spelling"
  else
    fail "WORK is pruned when TMPDIR reaches HOME by another spelling: exit $status"
    printf '%s\n' "$out" | grep -E '^FAIL|outside the prefix' | head -5 | sed 's/^/     /'
  fi

  # The same run must not have reported the prefix as an escapee, which is the
  # specific symptom: a pass above already implies it, but naming it means a
  # future failure says which bug came back.
  if printf '%s' "$out" | grep -q "created .* path(s) outside the prefix"; then
    fail "the prefix itself was reported as outside the prefix"
    printf '%s\n' "$out" | grep -A6 'outside the prefix' | head -8 | sed 's/^/     /'
  else
    pass "the prefix was not reported as outside the prefix"
  fi
fi

echo
if [ "$failures" -ne 0 ]; then
  echo "$checks checks, $failures failed"
  exit 1
fi
echo "$checks checks, all passed"
