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
WSHARP="${WSHARP:-$ROOT/target/debug/wsharp}"
CASES="${CASES:-$ROOT/tests/cases}"

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
