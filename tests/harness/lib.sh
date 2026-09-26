#!/usr/bin/env bash
# Shared bits for the adversarial harness.
#
# Every script here drives the *built compiler as a subprocess*, exactly as a
# user would, rather than linking against the crates. That is deliberate: the
# one seam a Rust test cannot see is the one where the binary, the runtime
# archive and `cc` meet, and it is also the only way to run the harness on a
# machine that has no cargo.

set -uo pipefail

# The repository root, whichever directory a script was started from.
harness_root() {
    cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd
}

ROOT="$(harness_root)"
CASES="${CASES:-$ROOT/tests/cases}"

# The compiler under test. Cargo names it `wsharp.exe` on Windows, so a harness
# that looked only for `wsharp` reported "no compiler" on the one release triple
# whose parity run is hardest to reproduce by hand -- and reported it as a
# harness error rather than as a platform the gate never covered.
if [ -z "${WSHARP:-}" ]; then
    if [ -x "$ROOT/target/debug/wsharp.exe" ]; then
        WSHARP="$ROOT/target/debug/wsharp.exe"
    else
        WSHARP="$ROOT/target/debug/wsharp"
    fi
fi

# `timeout(1)` is coreutils, and coreutils is **not in base macOS**. Two of the
# four release triples are macOS, so a script that spelled `timeout` inline
# would die with `command not found` on half the matrix -- or, worse, on a shell
# that tolerated it, run every case unbounded and turn a hang into a job that
# times out hours later with nothing to show. Resolve it once, here, and let
# `require_compiler` refuse to start rather than discover it per case.
# Homebrew installs the GNU one as `gtimeout`.
if [ -z "${TIMEOUT_BIN:-}" ]; then
    if command -v timeout >/dev/null 2>&1; then
        TIMEOUT_BIN=timeout
    elif command -v gtimeout >/dev/null 2>&1; then
        TIMEOUT_BIN=gtimeout
    else
        TIMEOUT_BIN=""
    fi
fi
# `fuzz.pl` reads it from the environment rather than re-deriving it, so the two
# halves of the harness cannot disagree about which binary bounds a run.
export TIMEOUT_BIN

# Where reports land. One directory per invocation so a night's run never
# overwrites the previous one, and so a comparison has two things to compare.
REPORTS="${WSHARP_HARNESS_REPORTS:-$ROOT/target/harness}"

require_compiler() {
    if [ ! -x "$WSHARP" ]; then
        echo "harness: no compiler at $WSHARP" >&2
        echo "harness: build one with \`cargo build --workspace\` (see CLAUDE.md:" >&2
        echo "harness: the runtime archive must be built too or AOT links a stale one)" >&2
        exit 2
    fi
    # Refused up front, not per case: an unbounded run of a corpus this size is
    # a job that dies on the runner's own timeout with no report written, which
    # looks like an infrastructure flake rather than a missing dependency.
    if [ -z "$TIMEOUT_BIN" ]; then
        echo "harness: no \`timeout\` and no \`gtimeout\` on PATH" >&2
        echo "harness: this harness bounds every run; install GNU coreutils" >&2
        echo "harness: (macOS: \`brew install coreutils\`, which provides gtimeout)" >&2
        exit 2
    fi
}

# The commit the compiler under test was built from, as far as the working tree
# can tell. Reported rather than trusted: a report that names a SHA the binary
# was not built from is worse than one that admits it does not know.
source_sha() {
    git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown
}

# The last commit that touched anything the compiler is built from. If this is
# older than the binary's mtime, the binary really is that source.
compiler_source_sha() {
    git -C "$ROOT" log -1 --format=%H -- crates 2>/dev/null || echo unknown
}

platform_line() {
    local libc=""
    if command -v ldd >/dev/null 2>&1; then
        libc="$(ldd --version 2>&1 | head -1)"
    fi
    printf '%s %s / %s\n' "$(uname -s)" "$(uname -m)" "${libc:-libc unknown}"
}

# A case's `// args:` line, split on whitespace exactly as the Rust harness
# does. Kept identical on purpose: two harnesses that disagree about a case's
# arguments disagree about what the case is.
case_args() {
    sed -n 's|^[[:space:]]*//[[:space:]]*args:[[:space:]]*||p' "$1" | head -1
}

# Whether a case must fail to compile.
case_expects_error() {
    grep -q '^[[:space:]]*//[[:space:]]*error:' "$1"
}

# Noise that differs between two runs of the same program for reasons that are
# not the compiler's fault, removed before two outputs are compared:
#
#   - the collector's statistics line (timings, and counts that legitimately
#     differ between the JIT and a built program),
#   - temporary directory and port numbers a case was told to invent,
#   - hexadecimal addresses in panic messages.
#
# Anything removed here is a thing this harness cannot check. The list is
# deliberately short for that reason.
normalise() {
    sed -e '/^W# gc: /d' \
        -e 's|/tmp/[A-Za-z0-9_.-]*|<tmp>|g' \
        -e 's|0x[0-9a-f][0-9a-f]*|<addr>|g' \
        -e 's|wsharp-[a-z]*-[0-9][0-9]*|<tmpdir>|g'
}
