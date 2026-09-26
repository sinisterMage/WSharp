#!/usr/bin/env bash
#
# Tests for `scripts/check-doc-examples.sh`.
#
# Each case builds a tiny repository in a scratch directory -- a README, a
# `tests/cases` or `examples` file -- and runs the checker against it. Driving
# the real script over real files is the point: the interesting failures are
# byte-level drift and a marker that names a file CI does not run, and neither
# can be tested against a mock.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly UNDER_TEST="$SCRIPT_DIR/../check-doc-examples.sh"

failures=0
checks=0

WORK="$(mktemp -d "${TMPDIR:-/tmp}/check-doc-examples-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# new_repo <name> -> prints the path to an empty scratch repository
new_repo() {
  local d="$WORK/$1"
  mkdir -p "$d/tests/cases" "$d/examples" "$d/docs"
  echo "$d"
}

# run_case <name> <repo> <wanted-exit> <wanted-substring>
run_case() {
  local name="$1" repo="$2" wanted_exit="$3" wanted_text="$4"
  local output status
  output="$(bash "$UNDER_TEST" "$repo" 2>&1)"
  status=$?
  checks=$((checks + 1))

  if [ "$status" -ne "$wanted_exit" ]; then
    echo "FAIL $name: exit $status, wanted $wanted_exit"
    echo "$output" | sed 's/^/       /'
    failures=$((failures + 1))
    return
  fi
  if [ -n "$wanted_text" ] && [[ "$output" != *"$wanted_text"* ]]; then
    echo "FAIL $name: output did not mention '$wanted_text'"
    echo "$output" | sed 's/^/       /'
    failures=$((failures + 1))
    return
  fi
  echo "ok   $name"
}

# --- a marked block that matches its file --------------------------------

r="$(new_repo matching)"
cat > "$r/examples/hello.ws" <<'WS'
fn main() void {
    print("hi");
}
WS
cat > "$r/README.md" <<'MD'
# Title

Some prose.

<!-- from: examples/hello.ws -->
```zig
fn main() void {
    print("hi");
}
```

More prose.
MD
run_case "a marked block identical to its file passes" "$r" 0 "gate 6a: ok"

# --- a marked block that has drifted -------------------------------------

r="$(new_repo drifted)"
cat > "$r/examples/hello.ws" <<'WS'
fn main() void {
    print("hi");
}
WS
cat > "$r/README.md" <<'MD'
<!-- from: examples/hello.ws -->
```zig
fn main() void {
    print("hello");
}
```
MD
run_case "a marked block that drifted fails" "$r" 1 "has drifted from 'examples/hello.ws'"

# --- drift of whitespace only, which a reader would never spot -----------

r="$(new_repo whitespace)"
printf 'fn main() void {\n    print("hi");\n}\n' > "$r/examples/hello.ws"
# Two spaces of indent in the document, four in the file.
cat > "$r/README.md" <<'MD'
<!-- from: examples/hello.ws -->
```zig
fn main() void {
  print("hi");
}
```
MD
run_case "indentation drift is caught" "$r" 1 "has drifted"

# --- a marker naming a file that does not exist --------------------------

r="$(new_repo missing)"
cat > "$r/README.md" <<'MD'
<!-- from: examples/nope.ws -->
```zig
fn main() void {}
```
MD
run_case "a marker naming a missing file fails" "$r" 1 "which does not exist"

# --- a marker naming a file no CI job executes ---------------------------

r="$(new_repo unexecuted)"
mkdir -p "$r/snippets"
printf 'fn main() void {\n    print("hi");\n}\n' > "$r/snippets/hello.ws"
cat > "$r/README.md" <<'MD'
<!-- from: snippets/hello.ws -->
```zig
fn main() void {
    print("hi");
}
```
MD
run_case "a file outside the executed directories fails" "$r" 1 "which no CI job executes"

# --- an unmarked complete program ---------------------------------------

r="$(new_repo unmarked)"
cat > "$r/README.md" <<'MD'
```zig
fn main() void {
    print("hi");
}
```
MD
run_case "an unmarked complete program fails" "$r" 1 "complete program with no marker"

# --- an unmarked fragment is fine ---------------------------------------

r="$(new_repo fragment)"
cat > "$r/README.md" <<'MD'
A type looks like this:

```zig
const Point = struct { x: i64, y: i64 };
```

and a call looks like this:

```zig
print(fib(10));
```
MD
run_case "unmarked fragments pass" "$r" 0 "2 fragment(s)"

# --- tests/cases counts as executed -------------------------------------

r="$(new_repo cases)"
printf 'fn main() void {\n    print("hi");\n}\n' > "$r/tests/cases/hello.ws"
cat > "$r/README.md" <<'MD'
<!-- from: tests/cases/hello.ws -->
```zig
fn main() void {
    print("hi");
}
```
MD
run_case "tests/cases counts as executed" "$r" 0 "gate 6a: ok"

# --- the wsharp fence language is covered too ---------------------------

r="$(new_repo wsharpfence)"
printf 'fn main() void {\n    print("hi");\n}\n' > "$r/examples/hello.ws"
cat > "$r/README.md" <<'MD'
<!-- from: examples/hello.ws -->
```wsharp
fn main() void {
    print("hi");
}
```
MD
run_case "a wsharp fence is checked as well as a zig one" "$r" 0 "gate 6a: ok"

# --- a marker separated by a blank line still applies -------------------

r="$(new_repo blankline)"
printf 'fn main() void {\n    print("hi");\n}\n' > "$r/examples/hello.ws"
printf '<!-- from: examples/hello.ws -->\n\n```zig\nfn main() void {\n    print("hi");\n}\n```\n' > "$r/README.md"
run_case "a blank line between marker and fence is allowed" "$r" 0 "gate 6a: ok"

# --- a marker does not carry across intervening prose -------------------

# The marker names the *next* block. Prose between them clears it, so the second
# block here is an unmarked complete program and must be reported -- otherwise a
# single marker could vouch for a block it never described.
r="$(new_repo notsticky)"
printf 'fn main() void {\n    print("hi");\n}\n' > "$r/examples/hello.ws"
cat > "$r/README.md" <<'MD'
<!-- from: examples/hello.ws -->
Some prose in between.

```zig
fn main() void {
    print("hi");
}
```
MD
run_case "prose between marker and fence clears the marker" "$r" 1 "complete program with no marker"

# --- docs/*.md is checked, not only README ------------------------------

r="$(new_repo docsdir)"
cat > "$r/docs/guide.md" <<'MD'
```zig
fn main() void {
    print("hi");
}
```
MD
cat > "$r/README.md" <<'MD'
# Title
MD
run_case "docs/*.md is checked too" "$r" 1 "docs/guide.md"

# --- an unclosed fence is reported rather than silently swallowing ------

r="$(new_repo unclosed)"
cat > "$r/README.md" <<'MD'
```zig
fn main() void {
    print("hi");
}
MD
run_case "an unclosed fence is reported" "$r" 1 "never closed"

# --- a shell fence is not a W# block -----------------------------------

r="$(new_repo shellfence)"
cat > "$r/README.md" <<'MD'
```sh
cargo test --workspace
```
MD
run_case "a sh fence is ignored" "$r" 0 "gate 6a: ok"

# ---------------------------------------------------------------------------

echo
if [ "$failures" -eq 0 ]; then
  echo "$checks checks, all passed"
  exit 0
fi
echo "$checks checks, $failures failed"
exit 1
