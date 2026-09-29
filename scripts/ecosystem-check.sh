#!/usr/bin/env bash
#
# Criterion 1, the `.wsharp` ecosystem check: one full run, on this machine.
#
#   WSHARP=target/release/wsharp INGOT=target/release/ingot \
#     scripts/ecosystem-check.sh [--work DIR] [--triple TRIPLE] [--keep]
#
# RELEASE-CRITERIA-1.0.md's soak table asks for "one full run per day for every
# day of the window, all exiting 0, on all four release triples". This is the
# run. `.github/workflows/ecosystem.yml` is the "per day" and the "four
# triples"; this file is what a run *is*, so that it means the same thing on a
# laptop as in CI.
#
# ## What it exercises
#
# Every program outside this repository that W# has a stake in, compiled by the
# candidate compiler the way its users compile it:
#
#   1. **sharpie**, the toolchain manager, at `main`: built with `wsharp build`,
#      then its own `tests/run.sh` (normally and under `WSHARP_GC_STRESS=1`) and
#      its `tests/rungs.sh`, the resolution ladder driven through the built
#      binary. Both are criterion 7's too; here they run every day rather than
#      when somebody remembers.
#   2. **Foundry**, the registry, at `main`: its own `ci/check.ws` -- which
#      imports `ingot/registry`, the reader `ingot resolve` uses -- over every
#      package, which checks the index's structure *and* fetches every release
#      and holds it to the tree hash the index promised. Then `ingot update`
#      and `ingot search` against the registry's URL, which is the one path a
#      user takes first and that a local directory skips.
#   3. **Every release the index does not mark yanked**, each in a fresh
#      project with a fresh `WSHARP_HOME`: `ingot init`, `add <name> =<ver>`,
#      `resolve`, `install`, `verify`; `wsharp check` and `wsharp build` of a
#      consumer that imports the package by name, and the built consumer run;
#      then the package's own offline tests, normally and under the collector's
#      stress mode, from a copy of the tree `ingot install` put in the store --
#      the bytes the registry hash names, not whatever the repository's HEAD
#      holds today.
#
# ## How a package's tests are run
#
# The packages do not agree, so the rule is "the way the package says":
#
#   tests/run.sh   is the package's harness. Run with `WSHARP` set, once plain
#                  and once with `WSHARP_GC_STRESS=1`, which is what
#                  `--gc-stress` sets and the only way to reach every
#                  `wsharp run` a foreign harness makes.
#   tests/run.py   the same, with the candidate first on `PATH` as `wsharp`
#                  (it calls it by name), and `--gc-stress` added to the second
#                  run as well as the variable.
#   neither        the W# case contract, applied here: every `tests/*.ws` with
#                  its `// expect:` lines against stdout, `// exit:` and
#                  `// env:` honoured, run plainly and with `--gc-stress`.
#                  Cases named `live*` are skipped because they need a database
#                  server, and so is each case in `SERVER_CASES` below, which
#                  needs one without saying so in its name. A release that
#                  leaves no case to run fails: a check that checked nothing
#                  is not a pass.
#
# ## Output
#
# One line per step on stdout, and nothing else there:
#
#     step<TAB>ok|fail<TAB>detail
#
# ending with a `summary` line. The same lines go to `results.tsv` in the work
# directory, beside one log per step under `logs/`. The last lines of a failing
# step's log go to stderr, so a CI log says why without an artifact download.
#
# ## Exit status
#
#   0  every step printed ok
#   1  a step failed; the summary line names every one
#   2  bad arguments
#
# **A network failure is a failure.** A clone gets three attempts, because
# git's transport is not what is under test; `ingot`'s own fetches get one,
# because `ingot/git`, `std/tls` and the collector under them are exactly what
# is under test, and a retry would turn a crash that happens one run in three
# into a green day. Nothing is ever reported as skipped: a soak that could not
# reach its subject did not soak it.
#
# ## Environment
#
#   WSHARP                 the candidate compiler. Default: this checkout's
#                          target/release/wsharp, then target/debug/wsharp.
#   INGOT                  ingot, bootstrapped from that compiler. Default:
#                          beside WSHARP.
#   ECOSYSTEM_CHECKOUT     the W# checkout the registry URL is read from.
#                          Default: the directory above this script's.
#   ECOSYSTEM_FOUNDRY_URL  the registry. Default: `DEFAULT_URL` in the
#                          checkout's crates/wsharp-runtime/src/ingot/registry.ws,
#                          so the URL has one definition.
#   ECOSYSTEM_SHARPIE_URL  default https://github.com/sinisterMage/sharpie.git
#   ECOSYSTEM_SHARPIE_REF, ECOSYSTEM_FOUNDRY_REF   default `main`
#   ECOSYSTEM_FETCH        a command that clones instead of git, for the
#                          selftest: called as `$cmd <url> <ref> <dest>`.
#   ECOSYSTEM_SERVER_CASES overrides SERVER_CASES below.
#   ECOSYSTEM_TIMEOUT_CAP  an upper bound on every per-command timeout, in
#                          seconds, for the selftest.
#   ECOSYSTEM_RETRY_DELAY  seconds between clone attempts, times the attempt
#                          number. Default 10.
#
# Bash 3.2 is the floor, because it is what macOS ships: no `mapfile`, no
# associative arrays, no `${var,,}`, and no arrays at all where one might be
# empty under `set -u`. Git Bash on Windows is the other target, which is why
# paths handed to a native program go through `native` and why every built
# program carries `$EXE`.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

usage() {
	sed -n '2,6p' "$0" >&2
	echo "See the header of $0 for the environment it reads." >&2
	exit 2
}

# --- arguments ---------------------------------------------------------------

WORK=""
TRIPLE=""
KEEP=0
while [ $# -gt 0 ]; do
	case "$1" in
	--work) [ $# -ge 2 ] || usage; WORK="$2"; shift 2 ;;
	--work=*) WORK="${1#--work=}"; shift ;;
	--triple) [ $# -ge 2 ] || usage; TRIPLE="$2"; shift 2 ;;
	--triple=*) TRIPLE="${1#--triple=}"; shift ;;
	--keep) KEEP=1; shift ;;
	-h | --help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
	*) echo "ecosystem-check: unknown argument $1" >&2; usage ;;
	esac
done

readonly RELEASE_TRIPLES="x86_64-unknown-linux-gnu x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-apple-darwin"

# Cases that need a server although their name does not say so, in packages
# with no harness of their own to say it for them. `package:path`, one per
# word. Each is a case that reads a server's address and a CA file out of the
# environment and has no default that could exist on a clean machine. Adding
# to this list is a decision somebody reviews; a package that grows another
# fails this check until it is either added here or given a harness.
SERVER_CASES="${ECOSYSTEM_SERVER_CASES-postgres/client:tests/tls.ws surrealdb/client:tests/tls.ws}"

# Seconds. Generous, because Windows and the Intel Mac are several times
# slower than the Linux runner and a suite under stress is slower again; the
# point of a bound is that a hang ends as a failure with a name, not a number
# anybody tunes.
T_CLONE=180
T_VERSION=60
T_BUILD=900
T_CHECK=600
T_INGOT=600
T_FOUNDRY=1200
T_SUITE=2700
T_CASE=900
T_RUN=120
CLONE_ATTEMPTS=3
RETRY_DELAY="${ECOSYSTEM_RETRY_DELAY:-10}"

SHARPIE_URL="${ECOSYSTEM_SHARPIE_URL:-https://github.com/sinisterMage/sharpie.git}"
SHARPIE_REF="${ECOSYSTEM_SHARPIE_REF:-main}"
FOUNDRY_REF="${ECOSYSTEM_FOUNDRY_REF:-main}"
CHECKOUT="${ECOSYSTEM_CHECKOUT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

# Nothing the caller's shell happens to hold may decide what a step does. Each
# step that wants one of these sets it for itself.
unset INGOT_REGISTRY WSHARP_HOME WSHARP_GC_STRESS SHARPIE_HOME SHARPIE_TOOLCHAIN \
	SHARPIE_RELEASE_DIR 2>/dev/null || true
export GIT_TERMINAL_PROMPT=0

# --- the candidate -----------------------------------------------------------

absolute() {
	case "$1" in
	/* | [A-Za-z]:[/\\]*) printf '%s' "$1" ;;
	*/*) printf '%s/%s' "$(cd "$(dirname "$1")" 2>/dev/null && pwd)" "$(basename "$1")" ;;
	*) command -v "$1" 2>/dev/null || printf '%s' "$1" ;;
	esac
}

if [ -z "${WSHARP:-}" ]; then
	for c in "$CHECKOUT/target/release/wsharp.exe" "$CHECKOUT/target/release/wsharp" \
		"$CHECKOUT/target/debug/wsharp.exe" "$CHECKOUT/target/debug/wsharp"; do
		if [ -f "$c" ]; then WSHARP="$c"; break; fi
	done
fi
WSHARP="$(absolute "${WSHARP:-wsharp}")"

# The suffix every built program carries, read off the one name already known
# rather than guessed from the platform -- sharpie's `tests/rungs.sh` does the
# same, for the same reason.
case "$WSHARP" in
*.exe) EXE=.exe ;;
*) EXE="" ;;
esac

if [ -z "${INGOT:-}" ]; then INGOT="$(dirname "$WSHARP")/ingot$EXE"; fi
INGOT="$(absolute "$INGOT")"

# How a native program spells a path this shell spells another way. Only Git
# Bash has two spellings: `/d/a/x` here is `D:/a/x` to `wsharp.exe`, and an
# environment variable is not always translated on the way across.
native() {
	if [ -n "$EXE" ] && command -v cygpath >/dev/null 2>&1; then
		cygpath -m "$1"
	else
		printf '%s' "$1"
	fi
}

guess_triple() {
	case "$(uname -s 2>/dev/null)-$(uname -m 2>/dev/null)" in
	Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
	Darwin-arm64) echo aarch64-apple-darwin ;;
	Darwin-x86_64) echo x86_64-apple-darwin ;;
	MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) echo x86_64-pc-windows-msvc ;;
	*) echo "unknown-$(uname -s 2>/dev/null)-$(uname -m 2>/dev/null)" ;;
	esac
}
TRIPLE_GIVEN="$TRIPLE"
[ -n "$TRIPLE" ] || TRIPLE="$(guess_triple)"

# --- the work directory ------------------------------------------------------

# A directory this script made is marked, and only a marked one is ever
# emptied: `--work /` must not be a way to lose a disk.
readonly MARK=".ecosystem-check-work"
if [ -z "$WORK" ]; then
	WORK="$(mktemp -d "${TMPDIR:-/tmp}/ecosystem.XXXXXX")" || { echo "mktemp failed" >&2; exit 2; }
	OWN_WORK=1
else
	OWN_WORK=0
	if [ -d "$WORK" ] && [ -n "$(ls -A "$WORK" 2>/dev/null)" ]; then
		if [ ! -f "$WORK/$MARK" ]; then
			echo "ecosystem-check: $WORK is not empty and was not made by this script" >&2
			exit 2
		fi
		rm -rf "$WORK"
	fi
	mkdir -p "$WORK" || exit 2
	WORK="$(cd "$WORK" && pwd)"
fi
: >"$WORK/$MARK"
LOGS="$WORK/logs"
RESULTS="$WORK/results.tsv"
mkdir -p "$LOGS" "$WORK/homes" "$WORK/projects" "$WORK/pkgtests"
: >"$RESULTS"

# shellcheck disable=SC2317 # reached through the trap
cleanup() {
	if [ "$OWN_WORK" -eq 1 ] && [ "$KEEP" -eq 0 ]; then
		rm -rf "$WORK"
	else
		echo "# work directory kept at $WORK" >&2
	fi
}
trap cleanup EXIT

# --- reporting ---------------------------------------------------------------

STEPS=0
FAILED=0
FAILED_NAMES=""

# Tabs separate the fields, so none may appear inside one.
flat() { printf '%s' "$1" | tr '\t\r\n' '   ' | sed 's/  */ /g; s/^ //; s/ $//'; }

result() {
	local step="$1" verdict="$2" detail
	detail="$(flat "${3:-}")"
	[ -n "$detail" ] || detail="-"
	STEPS=$((STEPS + 1))
	if [ "$verdict" != ok ]; then
		verdict=fail
		FAILED=$((FAILED + 1))
		FAILED_NAMES="$FAILED_NAMES $step"
	fi
	printf '%s\t%s\t%s\n' "$step" "$verdict" "$detail" | tee -a "$RESULTS"
}
pass() { result "$1" ok "${2:-}"; }
fail() { result "$1" fail "${2:-}"; }
not_run() { fail "$1" "not run: $2 failed"; }
say() { printf '# %s\n' "$*" >&2; }

# The end of a failing step's log, on stderr, so the CI log carries the reason.
explain() {
	[ -f "$1" ] || return 0
	{
		printf '#   --- the last 30 lines of %s\n' "$1"
		tail -n 30 "$1" | sed 's/^/#   | /'
	} >&2
}

logfile() { printf '%s/%s.log' "$LOGS" "$(printf '%s' "$1" | tr '/@: ' '____')"; }

# --- running things ----------------------------------------------------------

TIMEOUT=""
for t in timeout gtimeout; do
	# Proven rather than trusted: on Windows `timeout` can be System32's, which
	# waits for a keypress instead.
	if command -v "$t" >/dev/null 2>&1 && "$t" 5 true >/dev/null 2>&1 &&
		"$t" --version 2>&1 | grep -Eqi 'coreutils|gnu'; then
		TIMEOUT="$t"
		break
	fi
done

LAST_BOUND=0
bound() {
	local secs="$1"
	if [ -n "${ECOSYSTEM_TIMEOUT_CAP:-}" ] && [ "$ECOSYSTEM_TIMEOUT_CAP" -lt "$secs" ]; then
		secs="$ECOSYSTEM_TIMEOUT_CAP"
	fi
	LAST_BOUND="$secs"
}

# run_in DIR SECS LOG CMD... -- both streams appended to LOG.
run_in() {
	local dir="$1" log="$3" status
	bound "$2"
	shift 3
	printf '$ (in %s) %s\n' "$dir" "$*" >>"$log"
	(cd "$dir" && exec "$TIMEOUT" -k 10 "$LAST_BOUND" "$@") >>"$log" 2>&1
	status=$?
	case "$status" in
	124 | 137) printf '# timed out after %ss\n' "$LAST_BOUND" >>"$log" ;;
	esac
	return "$status"
}

# capture_in DIR SECS OUT LOG CMD... -- stdout to OUT alone, stderr to LOG.
capture_in() {
	local dir="$1" out="$3" log="$4" status
	bound "$2"
	shift 4
	printf '$ (in %s) %s\n' "$dir" "$*" >>"$log"
	(cd "$dir" && exec "$TIMEOUT" -k 10 "$LAST_BOUND" "$@") >"$out" 2>>"$log"
	status=$?
	case "$status" in
	124 | 137) printf '# timed out after %ss\n' "$LAST_BOUND" >>"$log" ;;
	esac
	return "$status"
}

# Why a command failed, in a few words: a timeout, or its status and the last
# thing it said.
reason() {
	local status="$1" log="$2" last
	case "$status" in
	124 | 137) printf 'timed out after %ss' "$LAST_BOUND"; return ;;
	esac
	last="$(grep -v '^[[:space:]]*$' "$log" 2>/dev/null | grep -v '^\$ (in ' | tail -n 1 | cut -c1-240)"
	printf 'exit %s: %s' "$status" "${last:-no output}"
}

# The last "N passed, M failed" line a harness printed, if it printed one.
tally() { grep -E '^[0-9]+ passed, [0-9]+ failed' "$1" 2>/dev/null | tail -n 1; }

# clone STEP URL REF DEST -- a shallow clone with bounded retries. Prints the
# step's line; returns 0 only on success.
clone() {
	local step="$1" url="$2" ref="$3" dest="$4" log attempt=1 status=1 head
	log="$(logfile "$step")"
	say "$step: $url ($ref)"
	while [ "$attempt" -le "$CLONE_ATTEMPTS" ]; do
		rm -rf "$dest"
		if [ -n "${ECOSYSTEM_FETCH:-}" ]; then
			run_in "$WORK" "$T_CLONE" "$log" "$ECOSYSTEM_FETCH" "$url" "$ref" "$dest"
		else
			run_in "$WORK" "$T_CLONE" "$log" git clone --quiet --depth 1 --single-branch \
				--branch "$ref" "$url" "$dest"
		fi
		status=$?
		# Exit 0 with nothing on disk is not a clone.
		if [ "$status" -eq 0 ] && [ -d "$dest" ]; then break; fi
		[ "$status" -ne 0 ] || status=1
		if [ "$attempt" -lt "$CLONE_ATTEMPTS" ]; then sleep $((RETRY_DELAY * attempt)); fi
		attempt=$((attempt + 1))
	done
	if [ "$status" -ne 0 ]; then
		fail "$step" "could not clone $url ($ref) in $CLONE_ATTEMPTS attempts; last: $(reason "$status" "$log")"
		explain "$log"
		return 1
	fi
	head="$(git -C "$dest" rev-parse HEAD 2>/dev/null || echo unknown)"
	pass "$step" "$url $ref at $head"
	return 0
}

# --- preflight ---------------------------------------------------------------

preflight() {
	local problems="" version="" log cc=""
	log="$(logfile preflight)"
	if [ -z "$TIMEOUT" ]; then
		problems="$problems; no GNU timeout or gtimeout on PATH (install coreutils): nothing here runs unbounded"
	fi
	if [ -n "$TRIPLE_GIVEN" ]; then
		case " $RELEASE_TRIPLES " in
		*" $TRIPLE_GIVEN "*) ;;
		*) problems="$problems; $TRIPLE_GIVEN is not one of the four release triples" ;;
		esac
	fi
	if [ ! -f "$WSHARP" ]; then
		problems="$problems; no compiler at $WSHARP (set WSHARP)"
	elif [ -n "$TIMEOUT" ] && ! capture_in "$WORK" "$T_VERSION" "$WORK/version.out" "$log" "$WSHARP" --version; then
		problems="$problems; $WSHARP --version failed"
	else
		version="$(head -n 1 "$WORK/version.out" 2>/dev/null)"
	fi
	if [ ! -f "$INGOT" ]; then
		problems="$problems; no ingot at $INGOT (bootstrap it: wsharp build --module ingot/main -o ingot)"
	elif [ -n "$TIMEOUT" ] && ! run_in "$WORK" "$T_VERSION" "$log" "$INGOT" help; then
		problems="$problems; $INGOT help failed"
	fi
	if [ -z "${ECOSYSTEM_FETCH:-}" ] && ! command -v git >/dev/null 2>&1; then
		problems="$problems; no git"
	fi
	# `wsharp build` links with one; so does sharpie's rung test.
	if [ -n "${CC:-}" ]; then cc="$CC"; else
		for c in cc clang gcc; do
			if command -v "$c" >/dev/null 2>&1; then cc="$c"; break; fi
		done
	fi
	[ -n "$cc" ] || problems="$problems; no C compiler (cc, clang, gcc or CC): wsharp build cannot link"
	if [ -z "${ECOSYSTEM_FOUNDRY_URL:-}" ]; then
		FOUNDRY_URL="$(sed -n 's/^pub const DEFAULT_URL = "\(.*\)";.*$/\1/p' \
			"$CHECKOUT/crates/wsharp-runtime/src/ingot/registry.ws" 2>/dev/null | head -n 1)"
		[ -n "$FOUNDRY_URL" ] ||
			problems="$problems; cannot read DEFAULT_URL from $CHECKOUT/crates/wsharp-runtime/src/ingot/registry.ws (set ECOSYSTEM_CHECKOUT or ECOSYSTEM_FOUNDRY_URL)"
	else
		FOUNDRY_URL="$ECOSYSTEM_FOUNDRY_URL"
	fi
	if [ -n "$problems" ]; then
		fail preflight "${problems#; }"
		explain "$log"
		return 1
	fi
	pass preflight "$version, $TRIPLE, cc=$cc, $TIMEOUT, work $WORK"
	return 0
}

# --- 1. sharpie --------------------------------------------------------------

# sharpie's harnesses run under the switches its own CI sets around them: a
# case says `// env: SHARPIE_HOME=/tmp/...` and asserts paths built from it, and
# MSYS would otherwise rewrite that before `wsharp.exe` saw it. All three
# names, because Git Bash and MSYS2 spell them differently. Harmless elsewhere.

sharpie_suite() {
	local step="$1" dir="$2" stress="$3" log status extra=""
	log="$(logfile "$step")"
	[ "$stress" -eq 0 ] || extra="WSHARP_GC_STRESS=1"
	say "$step"
	# shellcheck disable=SC2086 # `extra` is an assignment or nothing
	run_in "$dir" "$T_SUITE" "$log" env MSYS_NO_PATHCONV=1 'MSYS2_ARG_CONV_EXCL=*' \
		'MSYS2_ENV_CONV_EXCL=*' $extra WSHARP="$WSHARP" sh tests/run.sh
	status=$?
	if [ "$status" -eq 0 ]; then
		pass "$step" "tests/run.sh: $(tally "$log")"
	else
		fail "$step" "tests/run.sh: $(tally "$log"); $(reason "$status" "$log")"
		explain "$log"
	fi
}

sharpie() {
	local dir="$WORK/sharpie" log status version built=0
	if ! clone sharpie.clone "$SHARPIE_URL" "$SHARPIE_REF" "$dir"; then
		for s in build tests tests-stress rungs; do not_run "sharpie.$s" sharpie.clone; done
		return
	fi

	log="$(logfile sharpie.build)"
	say sharpie.build
	# The command its CI's `Build` step runs, and the binary `rungs.sh` drives.
	if run_in "$dir" "$T_BUILD" "$log" "$WSHARP" build src/main.ws -o "sharpie$EXE" &&
		capture_in "$dir" "$T_RUN" "$WORK/sharpie-version.out" "$log" "./sharpie$EXE" version; then
		version="$(head -n 1 "$WORK/sharpie-version.out")"
		pass sharpie.build "wsharp build src/main.ws: $version"
		built=1
	else
		status=$?
		fail sharpie.build "$(reason "$status" "$log")"
		explain "$log"
	fi

	sharpie_suite sharpie.tests "$dir" 0
	sharpie_suite sharpie.tests-stress "$dir" 1

	if [ "$built" -eq 0 ]; then
		not_run sharpie.rungs sharpie.build
		return
	fi
	log="$(logfile sharpie.rungs)"
	say sharpie.rungs
	run_in "$dir" "$T_SUITE" "$log" env MSYS_NO_PATHCONV=1 'MSYS2_ARG_CONV_EXCL=*' \
		'MSYS2_ENV_CONV_EXCL=*' WSHARP="$WSHARP" SHARPIE="./sharpie$EXE" sh tests/rungs.sh
	status=$?
	if [ "$status" -eq 0 ]; then
		pass sharpie.rungs "tests/rungs.sh: $(tally "$log")"
	else
		fail sharpie.rungs "tests/rungs.sh: $(tally "$log"); $(reason "$status" "$log")"
		explain "$log"
	fi
}

# --- 2. Foundry --------------------------------------------------------------

# Every release the index holds, read by `ingot/registry` itself rather than by
# a second TOML reader here -- the same argument Foundry's `ci/check.ws` makes
# for itself. It is also one more real program the candidate has to compile.
write_lister() {
	cat >"$WORK/releases.ws" <<'EOF'
// Every release a registry holds, one line each, read with `ingot/registry`
// -- the reader `ingot resolve` uses -- so this list and the client cannot
// disagree about what the index says. Written by scripts/ecosystem-check.sh.
//
//   release<TAB>name<TAB>version<TAB>live|yanked
//   bad<TAB>name<TAB>why
const fault = @import("ingot/fault");
const list = @import("std/list");
const os = @import("std/os");
const registry = @import("ingot/registry");
const semver = @import("ingot/semver");
const text = @import("std/str");

fn main() i64 {
    var dir = ".";
    for (os.args()) |a| { dir = a; }
    const f = fault.none();
    const ix = registry.open(f, dir) orelse {
        print(text.concat("bad\t-\t", f.message));
        return 1;
    };
    var bad = 0;
    for (registry.names(ix)) |name| {
        const g = fault.none();
        const p = registry.package(g, ix, name) orelse {
            var why = "has no readable package.toml";
            if (!g.ok) { why = g.message; }
            print(text.concat(text.concat("bad\t", name), text.concat("\t", why)));
            bad += 1;
            continue;
        };
        for (list.to_array(p.releases)) |r| {
            var state = "live";
            if (r.yanked) { state = "yanked"; }
            print(text.join([]str{ "release", name, semver.render(r.version), state }, "\t"));
        }
    }
    if (bad > 0) { return 1; }
    return 0;
}
EOF
}

TAB="$(printf '\t')"
FOUNDRY_DIR=""
RELEASES_ALL=0
SUITE_DETAIL=""

foundry_releases() {
	local log out status live yanked bad packages
	log="$(logfile foundry.releases)"
	out="$WORK/releases.out"
	say foundry.releases
	write_lister
	capture_in "$WORK" "$T_CHECK" "$out" "$log" "$WSHARP" run releases.ws -- "$(native "$FOUNDRY_DIR")"
	status=$?
	cat "$out" >>"$log"
	bad="$(grep "^bad$TAB" "$out" | cut -f2- | tr '\t\n' ': ' )"
	awk -F'\t' '$1 == "release" && $4 == "live" {print $2 "\t" $3}' "$out" >"$WORK/releases.tsv"
	live="$(grep -c . "$WORK/releases.tsv")"
	yanked="$(awk -F'\t' '$1 == "release" && $4 == "yanked"' "$out" | grep -c .)"
	RELEASES_ALL=$((live + yanked))
	packages="$(cut -f1 "$WORK/releases.tsv" | sort -u | grep -c .)"
	if [ "$status" -ne 0 ] || [ -n "$bad" ]; then
		fail foundry.releases "the index cannot be listed: ${bad:-$(reason "$status" "$log")}"
		explain "$log"
		: >"$WORK/releases.tsv"
		return 1
	fi
	if [ "$live" -eq 0 ]; then
		# The whole point of the check is the releases. An index with none left
		# to check -- empty, all yanked, or read as empty by a broken reader --
		# is a run that checked nothing, and that is not a pass.
		fail foundry.releases "the index lists no release that is not yanked ($yanked yanked): nothing to check is not a pass"
		return 1
	fi
	pass foundry.releases "$live release(s) of $packages package(s) to check; $yanked yanked, not checked"
	return 0
}

foundry_check() {
	local dir="$FOUNDRY_DIR" log out status paths="" f checked verified bad last home
	log="$(logfile foundry.check)"
	out="$WORK/foundry-check.out"
	home="$WORK/homes/foundry-check"
	say foundry.check
	for f in "$dir"/packages/*/*/versions.toml; do
		[ -f "$f" ] && paths="$paths ${f#"$dir"/}"
	done
	if [ -z "$paths" ]; then
		fail foundry.check "the index holds no packages/<owner>/<name>/versions.toml"
		return
	fi
	# The registry's own check, as its workflow runs it, but handed *every*
	# package rather than the ones a pull request touched: structure over the
	# whole index, and every release fetched at its revision and hashed. Plainly,
	# not under `--gc-stress`: Foundry's workflow adds that flag to dodge a
	# collector crash, and whether that crash is still there is exactly what a
	# daily run of the compiler should say.
	# shellcheck disable=SC2086 # the paths are registry names: no spaces
	capture_in "$dir" "$T_FOUNDRY" "$out" "$log" env WSHARP_HOME="$(native "$home")" \
		"$WSHARP" run ci/check.ws -- $paths
	status=$?
	cat "$out" >>"$log"
	checked="$(awk -F'\t' '$1 == "checked" {print $2}' "$out" | tail -n 1)"
	verified="$(grep -c "^verified$TAB" "$out")"
	bad="$(grep "^bad$TAB" "$out" | cut -f2- | tr '\t\n' ': ')"
	last="$(grep -v '^[[:space:]]*$' "$out" | tail -n 1)"
	if [ "$status" -ne 0 ] || [ "$last" != ok ] || [ -n "$bad" ]; then
		fail foundry.check "ci/check.ws: ${bad:-$(reason "$status" "$log")}"
		explain "$log"
	elif [ "${checked:-0}" -lt 1 ]; then
		fail foundry.check "ci/check.ws checked no package"
	elif [ "$RELEASES_ALL" -gt 0 ] && [ "$verified" -ne "$RELEASES_ALL" ]; then
		fail foundry.check "ci/check.ws verified $verified release(s) and the index lists $RELEASES_ALL"
	else
		pass foundry.check "ci/check.ws: $checked package(s) checked, $verified release(s) fetched and matched their tree hash"
	fi
}

foundry_remote() {
	local log out status home proj commit found local_count
	log="$(logfile foundry.remote)"
	out="$WORK/foundry-remote.out"
	home="$WORK/homes/remote"
	proj="$WORK/projects/remote"
	mkdir -p "$home" "$proj"
	say foundry.remote
	# `INGOT_REGISTRY` as a URL, so this is ingot's git client fetching the
	# index -- the first thing a new user's `ingot add` does, and the path a
	# local directory skips for every other step.
	capture_in "$proj" "$T_INGOT" "$out" "$log" env WSHARP_HOME="$(native "$home")" \
		INGOT_REGISTRY="$FOUNDRY_URL" "$INGOT" update
	status=$?
	cat "$out" >>"$log"
	commit="$(awk -F'\t' '$1 == "updated" {print $3}' "$out" | tail -n 1)"
	if [ "$status" -ne 0 ] || [ -z "$commit" ]; then
		fail foundry.remote "ingot update from $FOUNDRY_URL: $(reason "$status" "$log")"
		explain "$log"
		return
	fi
	capture_in "$proj" "$T_INGOT" "$out" "$log" env WSHARP_HOME="$(native "$home")" \
		INGOT_REGISTRY="$FOUNDRY_URL" "$INGOT" -q search
	status=$?
	cat "$out" >>"$log"
	found="$(awk -F'\t' '$1 == "found" {print $2}' "$out" | tail -n 1)"
	local_count=0
	if [ -n "$FOUNDRY_DIR" ] && [ -d "$FOUNDRY_DIR/packages" ]; then
		local_count="$(find "$FOUNDRY_DIR/packages" -name package.toml -type f | grep -c .)"
	fi
	if [ "$status" -ne 0 ] || [ -z "$found" ]; then
		fail foundry.remote "ingot search: $(reason "$status" "$log")"
		explain "$log"
	elif [ -n "$FOUNDRY_DIR" ] && [ "$found" -ne "$local_count" ]; then
		fail foundry.remote "ingot fetched the index at $commit and found $found package(s); the clone holds $local_count"
	else
		pass foundry.remote "ingot update fetched $FOUNDRY_URL at $commit; search found $found package(s)"
	fi
}

foundry() {
	FOUNDRY_DIR="$WORK/foundry"
	if ! clone foundry.clone "$FOUNDRY_URL" "$FOUNDRY_REF" "$FOUNDRY_DIR"; then
		FOUNDRY_DIR=""
		not_run foundry.releases foundry.clone
		not_run foundry.check foundry.clone
		foundry_remote
		return 1
	fi
	foundry_releases
	foundry_check
	foundry_remote
}

# --- 3. every release --------------------------------------------------------

needs_server() {
	case " $SERVER_CASES " in
	*" $1:$2 "*) return 0 ;;
	esac
	return 1
}

# The W# case contract, for a package with no harness of its own. Sets
# SUITE_DETAIL; returns 0 when every case that ran passed and at least one ran.
generic_suite() {
	local name="$1" dir="$2" stress="$3" log="$4" file base want got want_exit envs status
	local ran=0 passed=0 failures="" skipped="" out="$WORK/case.out" flag=""
	[ "$stress" -eq 0 ] || flag="--gc-stress"
	for file in "$dir"/tests/*.ws; do
		[ -f "$file" ] || continue
		base="${file##*/}"
		case "$base" in
		live*)
			skipped="$skipped $base"
			continue
			;;
		esac
		if needs_server "$name" "tests/$base"; then
			skipped="$skipped $base"
			continue
		fi
		want="$(sed -n 's|^// expect: \{0,1\}||p' "$file")"
		want_exit="$(sed -n 's|^// exit: *\([0-9][0-9]*\).*|\1|p' "$file" | head -n 1)"
		[ -n "$want_exit" ] || want_exit=0
		envs="$(sed -n 's|^// env: \{0,1\}||p' "$file" | tr '\n' ' ')"
		ran=$((ran + 1))
		# shellcheck disable=SC2086 # `envs` and `flag` are word lists
		capture_in "$dir" "$T_CASE" "$out" "$log" env $envs "$WSHARP" run $flag "tests/$base"
		status=$?
		got="$(tr -d '\r' <"$out")"
		{
			printf '%s\n' "$got" | sed 's/^/  stdout: /'
			printf '  exit %s\n' "$status"
		} >>"$log"
		if [ "$status" -eq "$want_exit" ] && [ "$got" = "$want" ]; then
			passed=$((passed + 1))
		elif [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then
			failures="$failures $base(timed out after ${LAST_BOUND}s)"
		elif [ "$status" -ne "$want_exit" ]; then
			failures="$failures $base(exit $status)"
		else
			failures="$failures $base(output differs)"
			{
				printf '  wanted:\n'
				printf '%s\n' "$want" | sed 's/^/    /'
			} >>"$log"
		fi
	done
	SUITE_DETAIL="W# case contract: $passed of $ran case(s) passed${failures:+; failed:$failures}${skipped:+; need a server, not run:$skipped}"
	if [ "$ran" -eq 0 ]; then
		SUITE_DETAIL="no offline case to run${skipped:+ (need a server:$skipped)}: a package test that ran nothing is not a pass"
		return 1
	fi
	[ -z "$failures" ]
}

CANDIDATE_BIN=""
PYTHON=""
# A directory holding the candidate as `wsharp`, for a harness that calls it by
# name. The candidate's own directory when it is already called that, which
# keeps the runtime archive beside it for a harness that builds.
candidate_bin() {
	if [ -n "$CANDIDATE_BIN" ]; then return; fi
	if [ "$(basename "$WSHARP")" = "wsharp$EXE" ]; then
		CANDIDATE_BIN="$(dirname "$WSHARP")"
	else
		CANDIDATE_BIN="$WORK/bin"
		mkdir -p "$CANDIDATE_BIN"
		ln -s "$WSHARP" "$CANDIDATE_BIN/wsharp$EXE" 2>/dev/null || cp "$WSHARP" "$CANDIDATE_BIN/wsharp$EXE"
	fi
}
find_python() {
	local p
	[ -z "$PYTHON" ] || return 0
	for p in python3 python; do
		if command -v "$p" >/dev/null 2>&1 &&
			"$p" -c 'import sys; sys.exit(0 if sys.version_info[0] == 3 else 1)' >/dev/null 2>&1; then
			PYTHON="$p"
			return 0
		fi
	done
	return 1
}

# package_suite STEP NAME ENTRY HOME STRESS -- the package's own offline tests.
package_suite() {
	local step="$1" name="$2" entry="$3" home="$4" stress="$5" log copy status harness
	local stress_env=""
	log="$(logfile "$step")"
	copy="$WORK/pkgtests/$(printf '%s' "$step" | tr '/@' '__')"
	say "$step"
	# A copy, fresh for each pass: the store entry is content-addressed and must
	# not be written to, and a harness may generate files into its own tree
	# (proxima's writes `tests/gen/`) that the second pass must not inherit.
	rm -rf "$copy"
	mkdir -p "$(dirname "$copy")"
	if ! cp -R "$entry" "$copy" 2>>"$log"; then
		fail "$step" "cannot copy the installed tree $entry"
		return
	fi
	chmod -R u+w "$copy" 2>/dev/null || true
	# Its own dependencies, installed where its tests will look for them, as its
	# author would have them. Nothing to do for a package with none, and cheap.
	local verb
	for verb in resolve install; do
		run_in "$copy" "$T_INGOT" "$log" env WSHARP_HOME="$(native "$home")" \
			INGOT_REGISTRY="$(native "$FOUNDRY_DIR")" "$INGOT" -q "$verb"
		status=$?
		if [ "$status" -ne 0 ]; then
			fail "$step" "ingot $verb, for the package's own dependencies: $(reason "$status" "$log")"
			explain "$log"
			return
		fi
	done
	[ "$stress" -eq 0 ] || stress_env="WSHARP_GC_STRESS=1"

	if [ -f "$copy/tests/run.sh" ]; then
		harness="tests/run.sh"
		# shellcheck disable=SC2086
		run_in "$copy" "$T_SUITE" "$log" env $stress_env WSHARP_HOME="$(native "$home")" \
			WSHARP="$WSHARP" sh tests/run.sh
		status=$?
		SUITE_DETAIL="$harness: $(tally "$log")"
		if [ "$status" -eq 0 ] && tally "$log" | grep -q '^0 passed'; then
			status=1
			SUITE_DETAIL="$harness ran no case: a package test that ran nothing is not a pass"
		fi
	elif [ -f "$copy/tests/run.py" ]; then
		harness="tests/run.py"
		if ! find_python; then
			fail "$step" "$harness needs Python 3 and there is none on PATH"
			return
		fi
		candidate_bin
		local py_flag=""
		[ "$stress" -eq 0 ] || py_flag="--gc-stress"
		# shellcheck disable=SC2086
		run_in "$copy" "$T_SUITE" "$log" env $stress_env WSHARP_HOME="$(native "$home")" \
			PATH="$CANDIDATE_BIN:$PATH" "$PYTHON" tests/run.py $py_flag
		status=$?
		SUITE_DETAIL="$harness: $(grep -v '^[[:space:]]*$' "$log" | grep -v '^\$ (in ' | tail -n 1 | cut -c1-120)"
	else
		harness="generic"
		generic_suite "$name" "$copy" "$stress" "$log"
		status=$?
	fi

	if [ "$status" -eq 0 ]; then
		pass "$step" "$SUITE_DETAIL"
	else
		if [ "$harness" = generic ]; then
			fail "$step" "$SUITE_DETAIL"
		else
			fail "$step" "$SUITE_DETAIL; $(reason "$status" "$log")"
		fi
		explain "$log"
	fi
}

one_release() {
	local name="$1" ver="$2" id slug home proj log status entry tree expected got
	id="$name@$ver"
	slug="$(printf '%s' "$id" | tr '/@' '__')"
	home="$WORK/homes/$slug"
	proj="$WORK/projects/$slug"
	rm -rf "$home" "$proj"
	mkdir -p "$home" "$proj"
	log="$(logfile "$id/install")"
	say "$id"

	# A fresh project and a fresh store, and the registry as the directory
	# already cloned. `=` is the pin: `ingot add name 1.2.0` would mean `^1.2.0`
	# and could choose a newer release than the one under test.
	local verb failed_verb=""
	for verb in "init eco/consumer" "add $name =$ver" "resolve" "install" "verify"; do
		# shellcheck disable=SC2086 # a verb and its arguments
		run_in "$proj" "$T_INGOT" "$log" env WSHARP_HOME="$(native "$home")" \
			INGOT_REGISTRY="$(native "$FOUNDRY_DIR")" "$INGOT" $verb
		status=$?
		if [ "$status" -ne 0 ]; then
			failed_verb="$verb"
			break
		fi
	done
	entry=""
	if [ -z "$failed_verb" ] && [ -f "$proj/ingot.env" ]; then
		entry="$(awk -F'\t' -v n="$name" '$1 == n {print $2; exit}' "$proj/ingot.env")"
	fi
	if [ -n "$failed_verb" ]; then
		fail "$id/install" "ingot ${failed_verb%% *}: $(reason "$status" "$log")"
		explain "$log"
	elif [ -z "$entry" ] || [ ! -d "$entry" ]; then
		fail "$id/install" "ingot install left no store entry for $name in ingot.env"
		explain "$log"
		entry=""
	else
		tree="$(awk -F'\t' -v n="$name" '$1 == "installed" && $2 == n {print $3}' "$log" | tail -n 1)"
		pass "$id/install" "init, add =$ver, resolve, install, verify: tree sha256:${tree:-?}"
	fi
	if [ -z "$entry" ]; then
		for s in check build tests tests-stress; do not_run "$id/$s" "$id/install"; done
		return
	fi

	# The smallest program that depends on the package the way a user's does:
	# by name, through the `ingot.env` install wrote.
	expected="ecosystem consumer of $name $ver"
	mkdir -p "$proj/src"
	cat >"$proj/src/consumer.ws" <<EOF
// Written by scripts/ecosystem-check.sh: a program that depends on
// $name $ver the way a user's does, by name, through ingot.env.
const pkg = @import("$name");

fn main() i64 {
    print("$expected");
    return 0;
}
EOF
	log="$(logfile "$id/check")"
	if run_in "$proj" "$T_CHECK" "$log" "$WSHARP" check src/consumer.ws; then
		pass "$id/check" "wsharp check of a consumer importing $name"
	else
		status=$?
		fail "$id/check" "$(reason "$status" "$log")"
		explain "$log"
	fi

	log="$(logfile "$id/build")"
	run_in "$proj" "$T_BUILD" "$log" "$WSHARP" build src/consumer.ws -o "consumer$EXE"
	status=$?
	if [ "$status" -eq 0 ]; then
		capture_in "$proj" "$T_RUN" "$WORK/consumer.out" "$log" "./consumer$EXE"
		status=$?
		if [ "$status" -ne 0 ]; then
			fail "$id/build" "the built consumer: $(reason "$status" "$log")"
			explain "$log"
		fi
	else
		fail "$id/build" "wsharp build: $(reason "$status" "$log")"
		explain "$log"
	fi
	if [ "$status" -eq 0 ]; then
		got="$(tr -d '\r' <"$WORK/consumer.out")"
		if [ "$got" = "$expected" ]; then
			pass "$id/build" "wsharp build, and the executable ran"
		else
			fail "$id/build" "the built consumer printed '$(flat "$got")', wanted '$expected'"
		fi
	fi

	package_suite "$id/tests" "$name" "$entry" "$home" 0
	package_suite "$id/tests-stress" "$name" "$entry" "$home" 1
}

releases() {
	local name ver
	[ -s "$WORK/releases.tsv" ] || return 0
	while IFS="$TAB" read -r name ver; do
		[ -n "$name" ] || continue
		one_release "$name" "$ver" </dev/null
	done <"$WORK/releases.tsv"
}

# --- the run -----------------------------------------------------------------

summary() {
	local line
	if [ "$FAILED" -eq 0 ]; then
		line="$(printf 'summary\tok\t%s steps, all ok, on %s' "$STEPS" "$TRIPLE")"
		printf '%s\n' "$line" | tee -a "$RESULTS"
		exit 0
	fi
	line="$(printf 'summary\tfail\t%s of %s steps failed, on %s:%s' "$FAILED" "$STEPS" "$TRIPLE" "$FAILED_NAMES")"
	printf '%s\n' "$line" | tee -a "$RESULTS"
	exit 1
}

main() {
	say "ecosystem check: $WSHARP on $TRIPLE, work in $WORK"
	if ! preflight; then summary; fi
	sharpie
	foundry
	releases
	summary
}

# One line, and the last. Bash reads a script as it runs it, so a run that
# outlives an edit to this file -- a `git pull` in the checkout an hour-long run
# is using -- would otherwise go on to execute whatever now sits at the byte
# offset it had reached. That happened while this was being written: the run
# ended without its summary line, and exited 0. With the steps in a function the
# whole run is parsed before the first one starts.
# shellcheck disable=SC2317 # `main` always exits; this is the guard if it ever does not
{ main; exit $?; }
