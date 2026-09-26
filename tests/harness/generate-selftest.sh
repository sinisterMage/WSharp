#!/usr/bin/env bash
# Does the grammar generator produce whole programs, reproducibly, and say what
# it covered?
#
# `generate.pl` is upstream of a fuzz campaign, which makes its failures the
# expensive kind: a generator that emits a syntax error produces a campaign where
# every input is refused with a diagnostic, every verdict is `OK`, and the report
# says "no findings" after spending a runner-hour proving that `fn` is a keyword.
# That is the same "passing for the wrong reason" shape `selftest.sh` and
# `conform-selftest.sh` exist for, one stage earlier in the pipeline.
#
# There is no W# compiler here and there does not need to be. Four properties are
# checkable from the text alone, and each one is a real failure mode:
#
#   - **Reproducibility.** A seed that means two things on two machines cannot
#     carry a finding between them.
#   - **Structural wholeness.** Balanced braces and parentheses, exactly one
#     `main`, and every top-level line actually being a declaration. That last
#     one is not hypothetical: an early version of `a_struct` returned a two-
#     element list from a body `prod` calls in *scalar* context, so the program
#     got a bare `S1` where the struct declaration belonged and every such
#     program was a parse error. This selftest is where that stays fixed.
#   - **Termination.** Every generated loop is bounded, and generation itself is
#     bounded by depth. A hang the generator caused is not a finding about W#.
#   - **Grammar coverage, reported.** The report lists every *declared*
#     production including the ones with a count of zero, because a production
#     that silently vanished from the report reads as absent by design.
#
# Usage:
#   tests/harness/generate-selftest.sh

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GEN="$HERE/generate.pl"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wsharp-generate-selftest-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

failures=0
ok()  { printf 'ok    %s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; failures=$((failures + 1)); }

# --- it compiles at all ---------------------------------------------------
if perl -c "$GEN" >/dev/null 2>&1; then
    ok "generate.pl compiles"
else
    bad "generate.pl does not compile"
    perl -c "$GEN" 2>&1 | sed 's/^/      /' >&2
    exit 1
fi

# --- reproducibility ------------------------------------------------------
a="$(perl "$GEN" --seed 4242 --print)"
b="$(perl "$GEN" --seed 4242 --print)"
if [ "$a" = "$b" ]; then
    ok "one seed is one program, twice"
else
    bad "seed 4242 produced two different programs"
fi

# Two seeds producing the same program would mean the seed is not doing
# anything, which would make a campaign one input repeated N times.
distinct="$(for s in 1 2 3 4 5 6 7 8 9 10; do perl "$GEN" --seed "$s" --print; echo '---'; done | sha256sum)"
same_count=0
for s in 1 2 3 4 5 6 7 8 9 10; do
    perl "$GEN" --seed "$s" --print >"$WORK/s$s.ws"
done
uniq_n="$(sha256sum "$WORK"/s*.ws | awk '{print $1}' | sort -u | wc -l | tr -d ' ')"
if [ "$uniq_n" -ge 8 ]; then
    ok "ten seeds gave $uniq_n distinct programs"
else
    bad "ten seeds gave only $uniq_n distinct programs; the seed is barely doing anything"
fi

# --- structural wholeness over a real sample ------------------------------
perl "$GEN" --seed 1 --count 120 --out "$WORK/corpus" --coverage >"$WORK/gen.log" 2>&1
gen_status=$?
if [ "$gen_status" -ne 0 ]; then
    bad "generating 120 programs exited $gen_status"
    cat "$WORK/gen.log" >&2
fi

n=0; unbalanced=0; no_main=0; two_main=0; stray=0; empty=0; unbounded_loop=0
for f in "$WORK/corpus"/*.ws; do
    [ -e "$f" ] || continue
    n=$((n + 1))
    [ -s "$f" ] || { empty=$((empty + 1)); continue; }

    opens="$(tr -cd '{' <"$f" | wc -c | tr -d ' ')"
    closes="$(tr -cd '}' <"$f" | wc -c | tr -d ' ')"
    lp="$(tr -cd '(' <"$f" | wc -c | tr -d ' ')"
    rp="$(tr -cd ')' <"$f" | wc -c | tr -d ' ')"
    [ "$opens" = "$closes" ] && [ "$lp" = "$rp" ] || unbalanced=$((unbalanced + 1))

    mains="$(grep -c '^fn main() i64 {' "$f")"
    [ "$mains" -eq 0 ] && no_main=$((no_main + 1))
    [ "$mains" -gt 1 ] && two_main=$((two_main + 1))

    # Every top-level line is a declaration, a comment, or blank. This is the
    # assertion that catches the bare-`S1` class: a fragment emitted where a
    # declaration belonged is a line at column 0 that is not one.
    # `}` and `};` close a declaration and are legitimately at column 0, so the
    # allowed set is the five things a top-level line can start with.
    if awk '/^[^[:space:]]/ && !/^(const |fn |\/\/|\}$|\};$)/ { bad = 1 }
            END { exit !bad }' "$f"; then
        stray=$((stray + 1))
    fi

    # A `while` the generator wrote must carry its own increment, or the campaign
    # reports HANG about a loop the harness wrote.
    while_n="$(grep -c 'while (' "$f")"
    incr_n="$(grep -cE '^\s+i[0-9]+ = i[0-9]+ \+ 1;' "$f")"
    [ "$while_n" -le "$incr_n" ] || unbounded_loop=$((unbounded_loop + 1))
done

if [ "$n" -eq 120 ]; then
    ok "120 programs written"
else
    bad "expected 120 programs, found $n"
fi
[ "$empty" -eq 0 ]          && ok "no program is empty"                  || bad "$empty empty program(s)"
[ "$unbalanced" -eq 0 ]     && ok "braces and parentheses balance in all $n" || bad "$unbalanced program(s) with unbalanced delimiters"
[ "$no_main" -eq 0 ]        && ok "every program has a main"             || bad "$no_main program(s) with no main"
[ "$two_main" -eq 0 ]       && ok "no program has two mains"             || bad "$two_main program(s) with more than one main"
[ "$stray" -eq 0 ]          && ok "every top-level line is a declaration" || bad "$stray program(s) with a fragment at column 0"
[ "$unbounded_loop" -eq 0 ] && ok "every generated loop carries its increment" || bad "$unbounded_loop program(s) with an unbounded loop"

# --- grammar coverage -----------------------------------------------------
COV="$WORK/corpus/coverage.md"
if [ -s "$COV" ]; then
    ok "a coverage report was written"
else
    bad "no coverage report at $COV"
fi

# The report must list every *declared* production, zero-count ones included.
declared="$(perl -ne 'print if /^our \@PRODUCTIONS/../^\);/' "$GEN" \
    | sed -e 's/our @PRODUCTIONS = qw(//' -e 's/);//' | tr -s ' \n' '\n' | grep -c '[a-z]')"
listed="$(grep -c '^| `[a-z_]*` | [0-9]* |$' "$COV")"
if [ "$listed" -eq "$declared" ]; then
    ok "the report lists all $declared declared productions"
else
    bad "the report lists $listed productions, $declared are declared -- a vanished one reads as absent by design"
fi

# And it must say how many were never reached, rather than only listing the ones
# that were. That number is the actionable one.
if grep -q 'Productions never generated: \*\*[0-9]* of [0-9]*\*\*' "$COV"; then
    ok "the report names how many productions were never generated"
else
    bad "the report does not say how many productions went unused"
fi

# A generator that reached only a handful of its own productions is not
# grammar-aware in any useful sense. Twelve is a floor, not a target.
reached="$(awk -F'|' '/^\| `[a-z_]*` \| [0-9]+ \|$/ { gsub(/ /,"",$3); if ($3+0 > 0) c++ } END { print c+0 }' "$COV")"
if [ "$reached" -ge 12 ]; then
    ok "120 programs reached $reached distinct productions"
else
    bad "120 programs reached only $reached productions"
fi

# Nesting is the half a mutator cannot report, so the pair table must exist and
# must contain something other than `toplevel`.
if awk -F'|' '/^\| `[a-z_]+` \| `[a-z_]+` \| [0-9]+ \|$/ { gsub(/[` ]/,"",$2); if ($2 != "toplevel") n++ } END { exit !(n >= 5) }' "$COV"; then
    ok "the report names nested (enclosing, nested) pairs"
else
    bad "the report has no construct-combination rows"
fi

# --- a run that generated nothing is not a clean run ----------------------
perl "$GEN" --seed 1 --count 0 --out "$WORK/none" >/dev/null 2>&1
[ $? -eq 2 ] && ok "--count 0 exits 2, not 0" || bad "--count 0 exited $?; an empty campaign read as clean"
perl "$GEN" --seed 1 --count 5 >/dev/null 2>&1
[ $? -eq 2 ] && ok "--count without --out exits 2" || bad "--count with nowhere to write exited $?"
perl "$GEN" --nonsense >/dev/null 2>&1
[ $? -eq 2 ] && ok "an unknown argument exits 2" || bad "an unknown argument exited $?"

# --- depth is bounded -----------------------------------------------------
# A generator whose recursion was not cut would either not return or return
# something enormous. Both are the harness hanging its own campaign.
big="$(perl "$GEN" --seed 5 --max-depth 9 --print | wc -c | tr -d ' ')"
if [ "$big" -gt 0 ] && [ "$big" -lt 2000000 ]; then
    ok "--max-depth 9 terminates and stays under 2 MB ($big bytes)"
else
    bad "--max-depth 9 produced $big bytes"
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "generate-selftest: the generator produces whole programs and says what it covered"
    exit 0
fi
echo "generate-selftest: $failures assertion(s) failed" >&2
exit 1
