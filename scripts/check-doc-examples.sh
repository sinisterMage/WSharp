#!/usr/bin/env bash
#
# Gate 6a: every documented example is a real file, executed in CI.
#
# A documented program that has drifted from the code is worse than no
# documentation, because a reader trusts it. So every fenced W# block in
# `README.md` and `docs/*.md` that is a complete program carries a marker naming
# the file it came from:
#
#     <!-- from: examples/fib.ws -->
#     ```zig
#     ...
#     ```
#
# and this script asserts three things:
#
#   1. A marked block is byte-identical to the file it names.
#   2. A named file is one the CI actually executes -- under `tests/cases/`,
#      which the case suite runs directly, or `examples/`, which the examples
#      job runs.
#   3. A block with no marker is a fragment. A complete program without a marker
#      is listed, because it is documentation nothing checks.
#
# "A complete program" is decided by one syntactic test -- the block declares
# `fn main` -- rather than by judgement, because a gate that needs a judgement
# call is not a gate.
#
# Run from the repository root:  bash scripts/check-doc-examples.sh [root]

set -uo pipefail

ROOT="${1:-.}"
cd "$ROOT" || { echo "error: cannot enter '$ROOT'" >&2; exit 3; }

# The fence languages a W# block is written with. `zig` because W#'s syntax is
# close enough that editors highlight it usefully, `wsharp` where the block is
# about W# specifically. A change here is a change to what this gate covers.
readonly WS_FENCES="zig wsharp"

# Directories whose `.ws` files CI executes. `tests/cases` is run by
# `crates/wsharp-cli/tests/cases.rs`, which runs every `.ws` directly in that
# directory; `examples` is run by the examples job in
# `.github/workflows/release-gates.yml`. Adding a directory here without adding
# the job that runs it would make this gate say something untrue.
readonly EXECUTED_DIRS="tests/cases examples"

problems=0
marked=0
fragments=0

problem() { problems=$((problems + 1)); echo "FAIL $*"; }
detail()  { echo "     $*"; }

is_executed() {
  local file="$1" dir
  for dir in $EXECUTED_DIRS; do
    case "$file" in
      "$dir"/*) return 0 ;;
    esac
  done
  return 1
}

# Pull out block N's body from a file, where N counts W# fenced blocks only.
# Done with awk rather than held in a shell variable so that a body containing
# backslashes, dollar signs or trailing whitespace survives byte-for-byte --
# which is the entire point of the comparison.
extract_block() {
  local doc="$1" want="$2"
  awk -v want="$want" -v fences="$WS_FENCES" '
    BEGIN { n = split(fences, f, " "); for (i = 1; i <= n; i++) lang["```" f[i]] = 1 }
    !inblock && ($0 in lang) { n_seen++; if (n_seen == want) { inblock = 1 } ; next }
    inblock && /^```[[:space:]]*$/ { exit }
    inblock { print }
  ' "$doc"
}

check_doc() {
  local doc="$1"
  local line marker="" n_seen=0 in_block=0 body_has_main=0 block_start=0
  local lineno=0

  while IFS= read -r line; do
    lineno=$((lineno + 1))

    if [ "$in_block" -eq 0 ]; then
      # A marker applies to the next fence. A blank line between the two is
      # allowed, so the source can be laid out readably; anything else clears
      # it, so a marker cannot silently attach to a block further down.
      case "$line" in
        '<!-- from:'*'-->')
          marker="${line#<!-- from:}"
          marker="${marker%-->}"
          # Trim.
          marker="${marker#"${marker%%[![:space:]]*}"}"
          marker="${marker%"${marker##*[![:space:]]}"}"
          continue
          ;;
      esac

      local is_ws_fence=0 f
      for f in $WS_FENCES; do
        [ "$line" = '```'"$f" ] && is_ws_fence=1
      done

      if [ "$is_ws_fence" -eq 1 ]; then
        in_block=1
        n_seen=$((n_seen + 1))
        body_has_main=0
        block_start=$lineno
        continue
      fi

      # Any other non-blank line breaks the marker's association.
      [ -n "$line" ] && marker=""
      continue
    fi

    # Inside a block.
    if [ "$line" = '```' ] || [[ "$line" =~ ^\`\`\`[[:space:]]*$ ]]; then
      in_block=0

      if [ -n "$marker" ]; then
        marked=$((marked + 1))
        if [ ! -f "$marker" ]; then
          problem "$doc:$block_start names '$marker', which does not exist"
        else
          # Compare the block body against the file, byte for byte.
          if extract_block "$doc" "$n_seen" | diff -q - "$marker" >/dev/null 2>&1; then
            if is_executed "$marker"; then
              :
            else
              problem "$doc:$block_start names '$marker', which no CI job executes"
              detail "executed directories are: $EXECUTED_DIRS"
            fi
          else
            problem "$doc:$block_start has drifted from '$marker'"
            extract_block "$doc" "$n_seen" | diff - "$marker" | head -12 \
              | while IFS= read -r d; do detail "$d"; done
          fi
        fi
      elif [ "$body_has_main" -eq 1 ]; then
        problem "$doc:$block_start is a complete program with no marker"
        detail "add '<!-- from: <path> -->' above the fence, and put the program"
        detail "in $EXECUTED_DIRS so that CI runs it"
      else
        fragments=$((fragments + 1))
      fi

      marker=""
      continue
    fi

    case "$line" in
      *"fn main"*) body_has_main=1 ;;
    esac
  done < "$doc"

  if [ "$in_block" -eq 1 ]; then
    problem "$doc: a fenced block beginning at line $block_start is never closed"
  fi
}

# ---------------------------------------------------------------------------

docs=()
[ -f README.md ] && docs+=("README.md")
for d in docs/*.md; do [ -f "$d" ] && docs+=("$d"); done

if [ "${#docs[@]}" -eq 0 ]; then
  echo "error: no README.md and no docs/*.md -- nothing to check, which is" >&2
  echo "       more likely a wrong working directory than an empty repository." >&2
  exit 3
fi

echo "check-doc-examples: ${#docs[@]} document(s)"
for doc in "${docs[@]}"; do
  check_doc "$doc"
done

# Every file under an executed directory that a document names must also still
# be reachable; the reverse -- a file no document names -- is fine, since not
# every case is documented.

echo
echo "  $marked marked block(s), $fragments fragment(s)"
if [ "$problems" -eq 0 ]; then
  echo "gate 6a: ok"
  exit 0
fi
echo "gate 6a: $problems problem(s)"
exit 1
