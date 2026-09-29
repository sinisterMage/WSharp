#!/usr/bin/env bash
#
# Tests for `scripts/ecosystem-check.sh`.
#
# The real check clones two repositories, fetches a registry and every package
# in it, and compiles all of it, which is an hour and a network. None of that is
# what these tests are about. They are about the *verdict*: that a run whose
# every step passed exits 0, that any step failing exits non-zero and names the
# step, that a clone which never succeeds is a failure line rather than a skip,
# and that an index with nothing in it to check is a failure rather than a
# vacuous pass -- the four ways a soak gate can lie, each one a way to write
# `ok` into the soak log on a day nothing was soaked.
#
# So the script runs for real, end to end, against stubs: a `wsharp` and an
# `ingot` that answer from fixtures, and an `ECOSYSTEM_FETCH` that builds the
# two repositories out of nothing. `STUB_FAIL` names what should go wrong, one
# word per fault:
#
#   version                  wsharp --version fails
#   clone-sharpie            every sharpie clone fails
#   clone-foundry            every Foundry clone fails
#   flaky-foundry            the first two Foundry clones fail
#   sharpie-build            wsharp build src/main.ws fails
#   sharpie-tests            sharpie's tests/run.sh fails
#   sharpie-stress           ... only with WSHARP_GC_STRESS=1
#   rungs                    sharpie's tests/rungs.sh fails
#   lister-bad               the release lister reports an unreadable entry
#   foundry-check            Foundry's ci/check.ws reports a hash mismatch
#   remote                   ingot update cannot reach the registry
#   remote-count             ingot search finds a different number of packages
#   install:<name>           ingot install of <name> cannot connect
#   check:<name>             wsharp check of a consumer of <name> fails
#   build:<name>             wsharp build of a consumer of <name> fails
#   consumer-output          a built consumer prints the wrong thing
#   case:<file>              wsharp run tests/<file> prints the wrong thing
#   stress-case:<file>       ... only under stress
#   hang:<file>              wsharp run tests/<file> never returns
#
# `STUB_INDEX` is the registry: one `name version live|yanked kind` per line,
# where kind is how that release's tests are laid out -- `generic` (cases and
# no harness), `server` (the same plus a live case and a case that needs a
# server), `runsh`, `runpy`, `liveonly` (nothing that runs offline) or `none`.
#
# Written for bash 3.2, like the script, because CI runs it on every triple.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly UNDER_TEST="$SCRIPT_DIR/../ecosystem-check.sh"

failures=0
checks=0

work="$(mktemp -d "${TMPDIR:-/tmp}/ecosystem-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT
stubs="$work/stubs"
mkdir -p "$stubs"

TAB="$(printf '\t')"

# --- the stubs ---------------------------------------------------------------

# Shared by the three: does STUB_FAIL name this fault?
cat >"$stubs/faults.sh" <<'EOF'
wants() {
  case " ${STUB_FAIL:-} " in *" $1 "*) return 0 ;; esac
  return 1
}
stressed() {
  [ "${WSHARP_GC_STRESS:-}" = 1 ] && return 0
  for a in "$@"; do [ "$a" = --gc-stress ] && return 0; done
  return 1
}
EOF

cat >"$stubs/wsharp" <<'EOF'
#!/usr/bin/env bash
. "$(dirname "$0")/faults.sh"
STRESSED=0
stressed "$@" && STRESSED=1
printf 'wsharp %s stress=%s\n' "$*" "$STRESSED" >>"$STUB_STATE/wsharp.calls"
verb="${1:-}"
[ $# -gt 0 ] && shift
case "$verb" in
--version)
  wants version && { echo "wsharp: cannot execute" >&2; exit 126; }
  echo "wsharp 0.0.0-stub"; exit 0 ;;
build)
  src="$1"; out="$3"
  case "$src" in
  src/main.ws)
    wants sharpie-build && { echo "error: sharpie does not compile" >&2; exit 1; }
    printf '#!/bin/sh\necho "sharpie 0.0.0-stub"\n' >"$out"; chmod +x "$out"; exit 0 ;;
  src/consumer.ws)
    name="$(sed -n 's/.*@import("\(.*\)");.*/\1/p' "$src")"
    wants "build:$name" && { echo "error: undefined reference to ws_stub" >&2; exit 1; }
    line="$(sed -n 's/.*print("\(.*\)");.*/\1/p' "$src")"
    wants consumer-output && line="something else"
    printf '#!/bin/sh\necho "%s"\n' "$line" >"$out"; chmod +x "$out"; exit 0 ;;
  esac
  exit 1 ;;
check)
  src="$1"
  name="$(sed -n 's/.*@import("\(.*\)");.*/\1/p' "$src")"
  # What the real loader does: a package name resolves through ingot.env.
  grep -q "^$name	" ingot.env 2>/dev/null || { echo "error: no package named $name" >&2; exit 1; }
  wants "check:$name" && { echo "error: type mismatch in $name" >&2; exit 1; }
  exit 0 ;;
run)
  [ "${1:-}" = --gc-stress ] && shift
  file="$1"; shift
  [ "${1:-}" = -- ] && shift
  case "$file" in
  releases.ws)
    wants lister-bad && { printf 'bad\tacme/broken\tversions.toml: not TOML\n'; exit 1; }
    dir="$1"
    [ -f "$dir/stub-index.tsv" ] || { printf 'bad\t-\t%s: is not a registry\n' "$dir"; exit 1; }
    awk '{printf("release\t%s\t%s\t%s\n", $1, $2, $3)}' "$dir/stub-index.tsv"
    exit 0 ;;
  ci/check.ws)
    printf 'registry\tFoundry\n'
    printf 'checked\t%s\n' "$(awk '{print $1}' stub-index.tsv | sort -u | grep -c .)"
    if wants foundry-check; then
      printf 'bad\tacme/demo 1.0.0\thashes to sha256:00, and the registry says sha256:11\n'
      printf 'failed\t1\n'; exit 1
    fi
    awk '{printf("verified\t%s %s\n", $1, $2)}' stub-index.tsv
    echo ok; exit 0 ;;
  tests/*)
    base="${file#tests/}"
    wants "hang:$base" && { sleep 60; exit 0; }
    grep -q '^// needs-server' "$file" && { echo "error.ConnectionRefused" >&2; exit 1; }
    if wants "case:$base" || { wants "stress-case:$base" && [ "$STRESSED" = 1 ]; }; then
      echo "not what was expected"; exit 0
    fi
    sed -n 's|^// expect: \{0,1\}||p' "$file"
    code="$(sed -n 's|^// exit: *||p' "$file")"
    exit "${code:-0}" ;;
  esac
  exit 1 ;;
esac
exit 1
EOF

cat >"$stubs/ingot" <<'EOF'
#!/usr/bin/env bash
. "$(dirname "$0")/faults.sh"
[ "${1:-}" = -q ] && shift
verb="${1:-}"
[ $# -gt 0 ] && shift
printf 'ingot %s %s\n' "$verb" "$*" >>"$STUB_STATE/ingot.calls"
kind_of() { awk -v n="$1" -v v="$2" '$1 == n && $2 == v {print $4}' "$INGOT_REGISTRY/stub-index.tsv"; }
tree() {
  d="$1"; kind="$2"
  mkdir -p "$d/src"
  printf '[package]\nname = "%s"\n' "$3" >"$d/ingot.toml"
  echo 'pub fn f() i64 { return 1; }' >"$d/src/lib.ws"
  case "$kind" in none) return ;; esac
  mkdir -p "$d/tests"
  case "$kind" in
  generic | server)
    printf '// expect: a ok\nfn main() i64 { return 0; }\n' >"$d/tests/a.ws"
    printf '// env: X=1\n// expect: b one\n// expect: b two\n' >"$d/tests/b.ws"
    printf '// exit: 3\n// expect: c\n' >"$d/tests/c.ws" ;;
  esac
  case "$kind" in
  server | liveonly)
    printf '// needs-server\n// expect: live passed\n' >"$d/tests/live.ws" ;;
  esac
  case "$kind" in
  server)
    printf '// needs-server\n// expect: tls passed\n' >"$d/tests/tls.ws" ;;
  runsh)
    printf '// expect: x ok\n' >"$d/tests/x.ws"
    cat >"$d/tests/run.sh" <<'H'
#!/bin/sh
got=$("$WSHARP" run tests/x.ws)
if [ "$got" = "x ok" ]; then echo "x.ws ... ok"; echo "1 passed, 0 failed"; exit 0; fi
echo "x.ws ... FAILED"; echo "0 passed, 1 failed"; exit 1
H
    ;;
  runpy)
    printf '// expect: y ok\n' >"$d/tests/y.ws"
    cat >"$d/tests/run.py" <<'H'
import subprocess, sys
out = subprocess.run(["wsharp", "run"] + (["--gc-stress"] if "--gc-stress" in sys.argv else []) + ["tests/y.ws"], capture_output=True, text=True).stdout
if out.strip() != "y ok":
    print("y failed: " + out.strip()); sys.exit(1)
print("all requested tests passed")
H
    ;;
  esac
}
case "$verb" in
help) echo "ingot -- stub"; exit 0 ;;
init) printf '[package]\nname = "%s"\n\n[dependencies]\n' "$1" >ingot.toml; printf 'wrote\tingot.toml\n' ;;
add) printf '%s\t%s\n' "$1" "$2" >>.stub-deps; printf 'added\t%s\n' "$1" ;;
resolve) : >ingot.lock; printf 'resolved\t%s\n' "$(grep -c . .stub-deps 2>/dev/null || echo 0)" ;;
install)
  [ -f ingot.lock ] || { echo "ingot: there is no ingot.lock here" >&2; exit 4; }
  printf 'root\t%s\tsrc/x.ws\n' "$PWD" >ingot.env
  if [ -f .stub-deps ]; then
    while IFS='	' read -r name req; do
      wants "install:$name" && { echo "ingot: cannot connect to github.com:443: connection refused" >&2; exit 4; }
      ver="${req#=}"
      entry="$WSHARP_HOME/store/$(printf '%s' "$name@$ver" | tr '/@' '__')"
      rm -rf "$entry"; tree "$entry" "$(kind_of "$name" "$ver")" "$name"
      printf 'installed\t%s\tfeedface\n' "$name"
      printf '%s\t%s\tsrc/lib.ws\n' "$name" "$entry" >>ingot.env
    done <.stub-deps
  fi
  printf 'ready\t1\n' ;;
verify) printf 'ready\t1\n' ;;
update)
  wants remote && { echo "ingot: cannot connect to github.com:443: connection timed out" >&2; exit 4; }
  printf 'updated\t%s\tdeadbeef\n' "$INGOT_REGISTRY" ;;
search)
  n="$(awk '{print $1}' "$STUB_INDEX" | sort -u | grep -c .)"
  wants remote-count && n=99
  printf 'found\t%s\n' "$n" ;;
*) exit 4 ;;
esac
exit 0
EOF

cat >"$stubs/fetch" <<'EOF'
#!/usr/bin/env bash
. "$(dirname "$0")/faults.sh"
url="$1"; dest="$3"
case "$url" in *sharpie*) kind=sharpie ;; *) kind=foundry ;; esac
n=$(( $(cat "$STUB_STATE/attempts-$kind" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$STUB_STATE/attempts-$kind"
if wants "clone-$kind" || { wants "flaky-$kind" && [ "$n" -lt 3 ]; }; then
  echo "fatal: unable to access '$url': Could not resolve host: github.com" >&2
  exit 128
fi
mkdir -p "$dest"
case "$kind" in
sharpie)
  mkdir -p "$dest/src" "$dest/tests"
  echo 'fn main() i64 { return 0; }' >"$dest/src/main.ws"
  cat >"$dest/tests/run.sh" <<'H'
#!/bin/sh
. "$STUB_DIR/faults.sh"
wants sharpie-tests && { echo "home.ws ... FAILED"; echo "7 passed, 1 failed"; exit 1; }
if [ "${WSHARP_GC_STRESS:-}" = 1 ] && wants sharpie-stress; then
  echo "home.ws ... FAILED (exit 134)"; echo "7 passed, 1 failed"; exit 1
fi
"$WSHARP" --version >/dev/null || exit 2
echo "8 passed, 0 failed"
H
  cat >"$dest/tests/rungs.sh" <<'H'
#!/bin/sh
. "$STUB_DIR/faults.sh"
"$SHARPIE" version >/dev/null || exit 2
wants rungs && { echo "a digest mismatch ... FAILED"; echo "13 passed, 1 failed"; exit 1; }
echo "14 passed, 0 failed"
H
  ;;
foundry)
  mkdir -p "$dest/ci" "$dest/packages"
  printf '[registry]\nversion = 1\nname = "Foundry"\n' >"$dest/Registry.toml"
  echo '// stub' >"$dest/ci/check.ws"
  cp "$STUB_INDEX" "$dest/stub-index.tsv"
  for name in $(awk '{print $1}' "$STUB_INDEX" | sort -u); do
    mkdir -p "$dest/packages/$name"
    printf '[package]\nname = "%s"\n' "$name" >"$dest/packages/$name/package.toml"
    awk -v n="$name" '$1 == n {printf("[[version]]\nversion = \"%s\"\n", $2)}' "$STUB_INDEX" \
      >"$dest/packages/$name/versions.toml"
  done ;;
esac
exit 0
EOF
chmod +x "$stubs/wsharp" "$stubs/ingot" "$stubs/fetch"

on_windows=0
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) on_windows=1 ;; esac
have_python=0
for p in python3 python; do
  if command -v "$p" >/dev/null 2>&1 && "$p" -c 'import sys; sys.exit(0 if sys.version_info[0] == 3 else 1)' 2>/dev/null; then
    have_python=1
  fi
done

# --- running it --------------------------------------------------------------

# index LINE... -- the registry for the next run.
index() {
  STUB_INDEX="$work/index-$checks.tsv"
  : >"$STUB_INDEX"
  local line
  for line in "$@"; do printf '%s\n' "$line" >>"$STUB_INDEX"; done
}

# The registry most scenarios use: every layout of tests, one yanked release,
# and a package whose server case is on the script's list.
default_index() {
  if [ "$have_python" -eq 1 ] && [ "$on_windows" -eq 0 ]; then
    index "acme/demo 1.0.0 live generic" "acme/demo 1.1.0 live runsh" "acme/old 0.1.0 yanked generic" \
      "postgres/client 0.1.0 live server" "acme/py 2.0.0 live runpy"
  else
    # A Python harness calls `wsharp` by name, which on Windows has to be an
    # `.exe` and here is a shell script -- so that layout is covered where it
    # can be, which is Linux and macOS.
    index "acme/demo 1.0.0 live generic" "acme/demo 1.1.0 live runsh" "acme/old 0.1.0 yanked generic" \
      "postgres/client 0.1.0 live server"
  fi
}

# run [ENV=VALUE...] [-- ARGS...] -- the script, against the stubs, with a
# fresh state directory. Sets OUT, ERR, STATUS and STATE.
run() {
  local envs=""
  while [ $# -gt 0 ] && [ "$1" != -- ]; do envs="$envs $1"; shift; done
  [ "${1:-}" = -- ] && shift
  STATE="$work/state-$checks"
  rm -rf "$STATE"
  mkdir -p "$STATE"
  # shellcheck disable=SC2086 # `envs` is a list of assignments
  OUT="$(env WSHARP="$stubs/wsharp" INGOT="$stubs/ingot" ECOSYSTEM_FETCH="$stubs/fetch" \
    ECOSYSTEM_FOUNDRY_URL=https://example.invalid/Foundry.git \
    ECOSYSTEM_SHARPIE_URL=https://example.invalid/sharpie.git \
    ECOSYSTEM_RETRY_DELAY=0 CC=stub-cc STUB_DIR="$stubs" STUB_STATE="$STATE" \
    STUB_INDEX="$STUB_INDEX" STUB_FAIL= $envs \
    bash "$UNDER_TEST" --work "$STATE/work" --triple x86_64-unknown-linux-gnu "$@" 2>"$STATE/stderr")"
  STATUS=$?
  ERR="$(cat "$STATE/stderr")"
}

problems=""
problem() { problems="$problems
      $*"; }

# Each run is also held to the invariants every run must keep, whatever it was
# testing: the output is only result lines, the last one is the summary, and
# the exit status agrees with the lines -- 0 exactly when no line failed.
invariants() {
  local bad_lines fails last
  bad_lines="$(printf '%s\n' "$OUT" | grep -Ev "^[^${TAB}]+${TAB}(ok|fail)${TAB}[^${TAB}]+\$")"
  [ -z "$bad_lines" ] || problem "lines that are not step<TAB>ok|fail<TAB>detail: $bad_lines"
  last="$(printf '%s\n' "$OUT" | tail -n 1)"
  case "$last" in "summary$TAB"*) ;; *) problem "the last line is not the summary: $last" ;; esac
  fails="$(printf '%s\n' "$OUT" | grep -c "$TAB""fail$TAB")"
  if [ "$fails" -eq 0 ] && [ "$STATUS" -ne 0 ]; then problem "no line failed and the exit status is $STATUS"; fi
  if [ "$fails" -ne 0 ] && [ "$STATUS" -eq 0 ]; then problem "$fails line(s) failed and the exit status is 0"; fi
  if [ -f "$STATE/work/results.tsv" ] && [ "$(cat "$STATE/work/results.tsv")" != "$OUT" ]; then
    problem "results.tsv is not what was printed"
  fi
}

has() { printf '%s\n' "$OUT" | grep -q -- "$1" || problem "no line matching /$1/"; }
lacks() { ! printf '%s\n' "$OUT" | grep -q -- "$1" || problem "a line matching /$1/"; }
exits() {
  case "$1" in
  0) [ "$STATUS" -eq 0 ] || problem "exit $STATUS, wanted 0" ;;
  nonzero) [ "$STATUS" -ne 0 ] || problem "exit 0, wanted non-zero" ;;
  esac
}

verdict() {
  checks=$((checks + 1))
  invariants
  if [ -n "$problems" ]; then
    failures=$((failures + 1))
    printf 'FAIL  %s%s\n' "$1" "$problems"
    printf '      --- stdout\n%s\n      --- stderr (tail)\n%s\n' "$OUT" "$(printf '%s\n' "$ERR" | tail -n 15)" |
      sed 's/^/      /'
  else
    printf 'ok    %s\n' "$1"
  fi
  problems=""
}

T="$TAB"

# --- everything passes -------------------------------------------------------

default_index
run
exits 0
has "^preflight${T}ok${T}wsharp 0.0.0-stub"
has "^sharpie.clone${T}ok"
has "^sharpie.build${T}ok${T}.*sharpie 0.0.0-stub"
has "^sharpie.tests${T}ok${T}tests/run.sh: 8 passed, 0 failed"
has "^sharpie.tests-stress${T}ok"
has "^sharpie.rungs${T}ok${T}tests/rungs.sh: 14 passed, 0 failed"
has "^foundry.clone${T}ok"
has "^foundry.releases${T}ok${T}.* 1 yanked"
has "^foundry.check${T}ok${T}.*checked"
has "^foundry.remote${T}ok"
has "^acme/demo@1.0.0/install${T}ok"
has "^acme/demo@1.0.0/check${T}ok"
has "^acme/demo@1.0.0/build${T}ok"
has "^acme/demo@1.0.0/tests${T}ok${T}W# case contract: 3 of 3 case(s) passed"
has "^acme/demo@1.0.0/tests-stress${T}ok"
has "^acme/demo@1.1.0/tests${T}ok${T}tests/run.sh: 1 passed, 0 failed"
has "^acme/demo@1.1.0/tests-stress${T}ok"
has "^postgres/client@0.1.0/tests${T}ok${T}.*need a server, not run: live.ws tls.ws"
has "^summary${T}ok"
lacks "${T}fail${T}"
# A yanked release is not a candidate for anybody, so it is not checked.
lacks "^acme/old@"
if [ "$have_python" -eq 1 ] && [ "$on_windows" -eq 0 ]; then
  has "^acme/py@2.0.0/tests${T}ok${T}tests/run.py: all requested tests passed"
  has "^acme/py@2.0.0/tests-stress${T}ok"
fi
# The stress pass is a stress pass: every compiler run of a stress step was
# stressed, and at least one of each kind happened.
[ "$(grep -c 'run --gc-stress tests/' "$STATE/wsharp.calls")" -ge 3 ] || problem "the generic stress pass did not pass --gc-stress"
grep -q 'run tests/x.ws.* stress=1' "$STATE/wsharp.calls" || problem "a harness's stress pass did not set WSHARP_GC_STRESS"
grep -q 'run tests/x.ws.* stress=0' "$STATE/wsharp.calls" || problem "a harness's plain pass was stressed"
# Pinned, not a caret: `add name 1.0.0` could choose 1.1.0.
grep -q 'ingot add acme/demo =1.0.0' "$STATE/ingot.calls" || problem "the release was not added pinned with ="
verdict "every step passing exits 0, and each step names itself"

# --- any failing step exits non-zero and names the step ----------------------

default_index
run STUB_FAIL=sharpie-build
exits nonzero
has "^sharpie.build${T}fail${T}exit 1: error: sharpie does not compile"
has "^sharpie.rungs${T}fail${T}not run: sharpie.build failed"
has "^sharpie.tests${T}ok"
has "^summary${T}fail${T}.* sharpie.build sharpie.rungs"
verdict "a sharpie that does not build fails, and so does the rung test it gates"

default_index
run STUB_FAIL=sharpie-stress
exits nonzero
has "^sharpie.tests${T}ok"
has "^sharpie.tests-stress${T}fail${T}tests/run.sh: 7 passed, 1 failed"
verdict "sharpie failing only under stress fails"

default_index
run STUB_FAIL=rungs
exits nonzero
has "^sharpie.rungs${T}fail${T}tests/rungs.sh: 13 passed, 1 failed"
verdict "a failing rung fails"

default_index
run STUB_FAIL=case:a.ws
exits nonzero
has "^acme/demo@1.0.0/tests${T}fail${T}.*2 of 3 case(s) passed; failed: a.ws(output differs)"
has "^acme/demo@1.0.0/tests-stress${T}fail"
has "^summary${T}fail${T}.*acme/demo@1.0.0/tests "
verdict "a package case printing the wrong thing fails, and names the case"

default_index
run STUB_FAIL=stress-case:b.ws
exits nonzero
has "^acme/demo@1.0.0/tests${T}ok"
has "^acme/demo@1.0.0/tests-stress${T}fail${T}.*b.ws(output differs)"
verdict "a case failing only under --gc-stress fails the stress step alone"

default_index
run STUB_FAIL=case:x.ws
exits nonzero
has "^acme/demo@1.1.0/tests${T}fail${T}tests/run.sh: 0 passed, 1 failed"
verdict "a package's own harness failing fails"

default_index
run STUB_FAIL=install:acme/demo
exits nonzero
has "^acme/demo@1.0.0/install${T}fail${T}ingot install: exit 4: ingot: cannot connect"
has "^acme/demo@1.0.0/tests${T}fail${T}not run: acme/demo@1.0.0/install failed"
has "^postgres/client@0.1.0/install${T}ok"
verdict "an install that cannot reach the network fails, and says so"

default_index
run STUB_FAIL=check:acme/demo
exits nonzero
has "^acme/demo@1.0.0/check${T}fail${T}.*type mismatch"
verdict "a consumer that does not type-check fails"

default_index
run STUB_FAIL=build:postgres/client
exits nonzero
has "^postgres/client@0.1.0/build${T}fail${T}wsharp build: exit 1"
verdict "a consumer that does not link fails"

default_index
run STUB_FAIL=consumer-output
exits nonzero
has "^acme/demo@1.0.0/build${T}fail${T}the built consumer printed 'something else'"
verdict "a built consumer that prints the wrong thing fails"

default_index
run STUB_FAIL=foundry-check
exits nonzero
has "^foundry.check${T}fail${T}ci/check.ws: acme/demo 1.0.0:hashes to"
verdict "the registry's own check failing fails"

default_index
run STUB_FAIL=remote
exits nonzero
has "^foundry.remote${T}fail${T}ingot update from .*: exit 4: ingot: cannot connect"
verdict "ingot failing to fetch the index fails"

default_index
run STUB_FAIL=remote-count
exits nonzero
has "^foundry.remote${T}fail${T}.*found 99 package(s); the clone holds"
verdict "ingot and git disagreeing about the index fails"

default_index
run STUB_FAIL=hang:a.ws ECOSYSTEM_TIMEOUT_CAP=2
exits nonzero
has "^acme/demo@1.0.0/tests${T}fail${T}.*a.ws(timed out after 2s)"
verdict "a case that never returns is a named timeout, not a hang"

# --- a failed clone is a fail line and non-zero -------------------------------

default_index
run STUB_FAIL=clone-foundry
exits nonzero
has "^foundry.clone${T}fail${T}could not clone https://example.invalid/Foundry.git (main) in 3 attempts; last: exit 128: fatal: unable to access"
has "^foundry.releases${T}fail${T}not run: foundry.clone failed"
has "^foundry.check${T}fail${T}not run: foundry.clone failed"
lacks "skip"
[ "$(cat "$STATE/attempts-foundry")" = 3 ] || problem "the clone was attempted $(cat "$STATE/attempts-foundry") time(s), not 3"
verdict "a Foundry clone that never succeeds fails after three attempts"

default_index
run STUB_FAIL=clone-sharpie
exits nonzero
has "^sharpie.clone${T}fail${T}.*3 attempts"
has "^sharpie.build${T}fail${T}not run: sharpie.clone failed"
has "^sharpie.rungs${T}fail${T}not run: sharpie.clone failed"
verdict "a sharpie clone that never succeeds fails, and so does everything after it"

default_index
run STUB_FAIL=flaky-foundry
exits 0
has "^foundry.clone${T}ok"
[ "$(cat "$STATE/attempts-foundry")" = 3 ] || problem "the clone was attempted $(cat "$STATE/attempts-foundry") time(s), not 3"
verdict "a clone that succeeds on its third attempt passes"

# --- a check that checked nothing does not pass ------------------------------

index
run
exits nonzero
has "^foundry.releases${T}fail${T}the index lists no release that is not yanked (0 yanked): nothing to check is not a pass"
has "^foundry.check${T}fail${T}the index holds no packages"
lacks "/install${T}"
verdict "an empty index fails"

index "acme/old 0.1.0 yanked generic" "acme/old 0.2.0 yanked generic"
run
exits nonzero
has "^foundry.releases${T}fail${T}.*(2 yanked): nothing to check is not a pass"
verdict "an index whose every release is yanked fails"

default_index
run STUB_FAIL=lister-bad
exits nonzero
has "^foundry.releases${T}fail${T}the index cannot be listed: acme/broken:versions.toml: not TOML"
lacks "/install${T}"
verdict "an index entry that cannot be read fails"

index "acme/live 1.0.0 live liveonly"
run
exits nonzero
has "^acme/live@1.0.0/tests${T}fail${T}no offline case to run (need a server: live.ws): a package test that ran nothing is not a pass"
verdict "a release with no offline case fails rather than passing vacuously"

index "acme/bare 1.0.0 live none"
run
exits nonzero
has "^acme/bare@1.0.0/tests${T}fail${T}no offline case to run"
verdict "a release with no tests at all fails"

# The server list is load-bearing: without it the same package fails, so it is
# not merely decorating a pass.
default_index
run ECOSYSTEM_SERVER_CASES=
exits nonzero
has "^postgres/client@0.1.0/tests${T}fail${T}.*tls.ws(exit 1)"
verdict "a server case not on the list is run, and fails"

# --- preflight ---------------------------------------------------------------

default_index
run WSHARP=/nonexistent/wsharp
exits nonzero
has "^preflight${T}fail${T}no compiler at /nonexistent/wsharp"
[ "$(printf '%s\n' "$OUT" | grep -c .)" -eq 2 ] || problem "more than the preflight and the summary were printed"
verdict "no compiler fails before anything is cloned"

default_index
run STUB_FAIL=version
exits nonzero
has "^preflight${T}fail${T}.*--version failed"
verdict "a compiler that cannot say its version fails"

default_index
run -- --triple x86_64-unknown-freebsd
exits nonzero
has "^preflight${T}fail${T}x86_64-unknown-freebsd is not one of the four release triples"
verdict "a triple that is not a release triple fails"

# --- an edit to the script while it runs does not change the run -------------

# Bash reads a script as it executes it. A run that outlives an in-place edit --
# an editor saving, a checkout being updated under an hour-long run -- goes on
# reading at a byte offset that now means something else. It happened while
# this check was written: the run lost its summary line and exited 0. So a copy
# is edited mid-run here, every offset shifted, and the run must still finish
# as it began.
default_index
copy="$work/edited/scripts"
mkdir -p "$copy"
cp "$UNDER_TEST" "$copy/ecosystem-check.sh"
STATE="$work/state-edited"
mkdir -p "$STATE"
env WSHARP="$stubs/wsharp" INGOT="$stubs/ingot" ECOSYSTEM_FETCH="$stubs/fetch" \
  ECOSYSTEM_FOUNDRY_URL=https://example.invalid/Foundry.git \
  ECOSYSTEM_SHARPIE_URL=https://example.invalid/sharpie.git \
  ECOSYSTEM_RETRY_DELAY=0 ECOSYSTEM_TIMEOUT_CAP=3 CC=stub-cc STUB_DIR="$stubs" \
  STUB_STATE="$STATE" STUB_INDEX="$STUB_INDEX" STUB_FAIL=hang:a.ws \
  bash "$copy/ecosystem-check.sh" --work "$STATE/work" --triple x86_64-unknown-linux-gnu \
  >"$STATE/out" 2>"$STATE/stderr" &
pid=$!
waited=0
while ! grep -q "^acme/demo@1.0.0/build" "$STATE/out" 2>/dev/null && [ "$waited" -lt 600 ]; do
  sleep 0.1
  waited=$((waited + 1))
done
# Rewritten in place, as an editor does -- the same inode, every byte moved.
{
  i=0
  while [ "$i" -lt 40 ]; do echo "# an edit made while the check was running"; i=$((i + 1)); done
  cat "$copy/ecosystem-check.sh"
} >"$copy/edited.tmp"
cat "$copy/edited.tmp" >"$copy/ecosystem-check.sh"
wait "$pid"
STATUS=$?
OUT="$(cat "$STATE/out")"
ERR="$(cat "$STATE/stderr")"
[ "$waited" -lt 600 ] || problem "the run never reached the packages, so nothing was edited mid-run"
exits nonzero
has "^acme/demo@1.0.0/tests${T}fail${T}.*timed out"
has "^postgres/client@0.1.0/tests-stress${T}"
has "^summary${T}fail"
verdict "an edit to the script mid-run does not change the rest of the run"

# ---------------------------------------------------------------------------

printf '\n%s of %s checks passed\n' "$((checks - failures))" "$checks"
[ "$failures" -eq 0 ]
