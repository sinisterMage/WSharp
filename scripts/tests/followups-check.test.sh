#!/usr/bin/env bash
#
# Tests for `scripts/followups-check.sh`.
#
# Criterion 4 is the one gate whose data is not in this tree: whether an issue is
# open is a question for GitHub. So this file's job is bigger than the other
# checks' selftests, and it has two halves.
#
# The first is the ordinary one -- the check must refuse an open issue, and refuse
# a closed one whose entry is missing or incomplete. A drift check that only ever
# passes is the drift it was meant to catch, one level up.
#
# The second is the half a green CI run will never show anybody: **the fetch
# failing.** No token, a rate limit, a dropped connection. Every one of those must
# exit 2 and say so, and none of them may exit 0, because a gate that reports
# "met" because it could not ask is worse than no gate at all -- it is the stack
# walk that finds no roots and so passes every root check. Those cases are driven
# by a stub fetcher, which is what `FOLLOWUPS_FETCH` exists for.
#
# Every case builds a throwaway tree with its own two documents in it. The
# live gate runs separately, so this selftest is deterministic and offline.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly UNDER_TEST="$SCRIPT_DIR/../followups-check.sh"

failures=0
checks=0

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# --- fixtures ---------------------------------------------------------------

# entry <number> <heading> [omit-field...] -- one LIMITATIONS.md entry, carrying
# all five fields except those named.
entry() {
  local number="$1" heading="$2"
  shift 2
  local omitted=" $* "

  echo "## $heading"
  echo
  [[ "$omitted" != *" What "* ]] && { echo "**What.** It does not work."; echo; }
  [[ "$omitted" != *" Why "* ]] && { echo "**Why.** The fix is more than an afternoon."; echo; }
  [[ "$omitted" != *" Workaround "* ]] && { echo "**Workaround.** None."; echo; }
  [[ "$omitted" != *" Disposition "* ]] && { echo "**Disposition: documented limitation.** Decided."; echo; }
  [[ "$omitted" != *" Tracked "* ]] && {
    echo "**Tracked.** [#$number](https://github.com/sinisterMage/WSharp/issues/$number), closed."
    echo
  }
}

# tree <name> <criterion-4-numbers> -- a tree whose LIMITATIONS.md is read from
# stdin, and whose criteria document names the given issue numbers.
tree() {
  local dir="$work/$1" numbers="$2"
  mkdir -p "$dir"

  {
    echo "# What W# does not do"
    echo
    echo "## The six, and what was decided"
    echo
    # The front matter names every subject and is an entry for none of them.
    # Every fixture carries it, because telling it apart from a real entry is
    # something the check has to do on the real file too.
    echo "| Issue | Limitation |"
    echo "|---|---|"
    local n
    for n in $numbers; do
      echo "| [#$n](https://github.com/sinisterMage/WSharp/issues/$n) | something |"
    done
    echo
    echo "# Language"
    echo
    cat
  } >"$dir/LIMITATIONS.md"

  {
    echo "## 3. Every fix carries a guard"
    echo
    echo "Nothing here names an issue."
    echo
    echo "## 4. Every remaining limitation is resolved or documented"
    echo
    printf 'The issues labelled `v1.0-limitation` —'
    for n in $numbers; do printf ' #%s,' "$n"; done
    echo " — plus any that join them."
    echo
    echo "## 5. Conformance"
    echo
    echo "Mentions #999, which is in another criterion and must not be picked up."
  } >"$dir/RELEASE-CRITERIA-1.0.md"

  echo "$dir"
}

# stub <name> <line...> -- a fetcher answering `list` with the given TSV rows and
# `get N` with whichever of them matches.
stub() {
  local path="$work/$1.sh"
  shift
  {
    echo '#!/usr/bin/env bash'
    echo 'rows=$(cat <<'"'"'EOF'"'"''
    local row
    for row in "$@"; do
      printf '%s\n' "$row"
    done
    echo 'EOF'
    echo ')'
    echo 'case "$1" in'
    echo '  list) printf "%s\n" "$rows" ;;'
    echo '  get) printf "%s\n" "$rows" | grep "^$2	" || exit 1 ;;'
    echo '  *) exit 1 ;;'
    echo 'esac'
  } >"$path"
  chmod +x "$path"
  echo "$path"
}

# A TSV row. Tabs, and no field ever empty -- see the script's JQ_ROWS comment.
row() {
  printf '%s\t%s\t%s\t%s\t%s' "$1" "$2" "$3" "$4" "$5"
}

# expect <name> <dir> <fetcher> <wanted-exit> <wanted-substring>
expect() {
  local name="$1" dir="$2" fetcher="$3" wanted_exit="$4" wanted_text="$5"
  local output status

  output="$(FOLLOWUPS_FETCH="$fetcher" bash "$UNDER_TEST" "$dir" 2>&1)"
  status=$?

  checks=$((checks + 1))

  if [ "$status" -ne "$wanted_exit" ]; then
    echo "FAIL $name: exit $status, wanted $wanted_exit"
    echo "     output: $output"
    failures=$((failures + 1))
    return
  fi

  if [ -n "$wanted_text" ] && [[ "$output" != *"$wanted_text"* ]]; then
    echo "FAIL $name: output did not mention '$wanted_text'"
    echo "     output: $output"
    failures=$((failures + 1))
    return
  fi

  echo "ok   $name"
}

# --- both allowed states ----------------------------------------------------

d="$(tree documented "9 10")"
entry 9 'A computed const is rejected' >>"$d/LIMITATIONS.md"
entry 10 'A field access needs a type' >>"$d/LIMITATIONS.md"
f="$(stub two_documented \
  "$(row 9 closed completed v1.0-limitation 'computed const')" \
  "$(row 10 closed completed v1.0-limitation 'field access')")"
expect "two closed and documented" "$d" "$f" 0 "2 of 2 subjects"

# Another issue's entry mentioning this one is not this one's entry. The real
# file's entry for #47 says on its Tracked line that it is the string case of
# #9 and #12; matching any mention made it the entry for all three, so deleting
# #9's own section still read as met.
d="$(tree cross_reference "9 47")"
{
  echo "## A top-level const array cannot hold strings"
  echo
  echo "**What.** It does not work."
  echo
  echo "**Why.** The fix is more than an afternoon."
  echo
  echo "**Workaround.** None."
  echo
  echo "**Disposition: documented limitation.** Decided."
  echo
  echo "**Tracked.** [#47](https://github.com/sinisterMage/WSharp/issues/47), the string"
  echo "case of [#9](https://github.com/sinisterMage/WSharp/issues/9)."
  echo
} >>"$d/LIMITATIONS.md"
f="$(stub cross_reference \
  "$(row 9 closed completed v1.0-limitation 'computed const')" \
  "$(row 47 closed completed v1.0-limitation 'string const array')")"
expect "a mention in another entry's Tracked line is not an entry" "$d" "$f" 1 "#9 is closed as a v1.0-limitation and no entry"

# Closed as fixed is the criterion's other allowed state, and it needs no entry:
# the label is gone because the limitation is.
d="$(tree fixed "9")"
f="$(stub fixed "$(row 9 closed completed 'defect,change:fix' 'computed const')")"
expect "closed as fixed, with no entry" "$d" "$f" 0 "closed as fixed"

# --- an open issue is a failure ---------------------------------------------

d="$(tree open "9")"
entry 9 'A computed const is rejected' >>"$d/LIMITATIONS.md"
f="$(stub open "$(row 9 open unset v1.0-limitation 'computed const')")"
expect "an open issue, entry and all" "$d" "$f" 1 "#9 is open"

# --- closed with no entry ---------------------------------------------------

d="$(tree unentered "9")"
f="$(stub unentered "$(row 9 closed completed v1.0-limitation 'computed const')")"
expect "closed on a comment, with no entry" "$d" "$f" 1 "no entry in LIMITATIONS.md names it"

# The front matter table names every issue and must not be mistaken for its
# entry -- if it were, every one of these would pass on the index alone.
expect "the index table is not an entry" "$d" "$f" 1 "CRITERION 4 NOT MET"

# --- an entry that is there and unfinished ----------------------------------

d="$(tree thin "9")"
entry 9 'A computed const is rejected' Why Workaround >>"$d/LIMITATIONS.md"
f="$(stub thin "$(row 9 closed completed v1.0-limitation 'computed const')")"
expect "an entry missing two fields" "$d" "$f" 1 "which is missing: Why Workaround"

# --- `#1` must not match `#15` ----------------------------------------------

d="$(tree prefix "1 15")"
entry 15 'A P-521 key' >>"$d/LIMITATIONS.md"
f="$(stub prefix \
  "$(row 1 closed completed v1.0-limitation 'the first issue')" \
  "$(row 15 closed completed v1.0-limitation 'a P-521 key')")"
expect "a number that is another's prefix" "$d" "$f" 1 "#1 is closed"

# --- a hash inside a fenced code block is not a heading ---------------------
#
# The real `x86-64 and aarch64 only` entry quotes `#[cfg(not(any(...)))]`, and a
# looser heading test cut that section in half above its **Tracked.** line.
d="$(tree fenced "11")"
{
  echo '## x86-64 and aarch64 only'
  echo
  echo '**What.** No other architecture.'
  echo
  echo '```rust'
  echo '#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]'
  echo '# a comment that starts with a hash and a space'
  echo '```'
  echo
  echo '**Why.** The collector walks frame pointers.'
  echo
  echo '**Workaround.** None.'
  echo
  echo '**Disposition: documented limitation.** A scope statement.'
  echo
  echo '**Tracked.** [#11](https://github.com/sinisterMage/WSharp/issues/11), closed.'
} >>"$d/LIMITATIONS.md"
f="$(stub fenced "$(row 11 closed completed v1.0-limitation 'two architectures')")"
expect "a section whose code block starts lines with a hash" "$d" "$f" 0 "1 of 1 subjects"

# --- the roster is discovered, and cannot shrink ----------------------------

# A seventh nobody wrote down: it is in the label's answer, so it is checked.
d="$(tree seventh "9")"
entry 9 'A computed const is rejected' >>"$d/LIMITATIONS.md"
f="$(stub seventh \
  "$(row 9 closed completed v1.0-limitation 'computed const')" \
  "$(row 21 closed completed v1.0-limitation 'a seventh, freshly labelled')")"
expect "a seventh that joined the label" "$d" "$f" 1 "#21 is closed"

# The label taken off an issue the criteria document still names. It has to have
# been *fixed*; closed as not planned is the quiet third state the criterion
# forbids, and dropping it from the roster would be how that goes unnoticed.
d="$(tree unlabelled "9 10")"
entry 9 'A computed const is rejected' >>"$d/LIMITATIONS.md"
f="$(stub unlabelled \
  "$(row 9 closed completed v1.0-limitation 'computed const')" \
  "$(row 10 closed not_planned 'wontfix' 'field access')")"
expect "the label removed, and closed as not planned" "$d" "$f" 1 "neither of the two states"

# Unknown closure metadata is not evidence of a fix.
d="$(tree unknown_reason "9")"
f="$(stub unknown_reason "$(row 9 closed unset '-' 'computed const')")"
expect "unknown closure reason is not fixed" "$d" "$f" 2 "cannot confirm"

# Exercise the real adapter without network, with a successful anonymous reply.
mkdir -p "$work/bin"
cat >"$work/bin/curl" <<'CURL'
#!/usr/bin/env bash
printf '%s\n' '[{"number":9,"state":"closed","state_reason":"completed","labels":[],"title":"fixed"}]' '200'
CURL
chmod +x "$work/bin/curl"
PATH="$work/bin:$PATH" GITHUB_TOKEN= GH_TOKEN= expect "missing token fails closed" "$d" "" 2 "GITHUB_TOKEN or GH_TOKEN is required"

# --- the fetch failing: every path must be exit 2, and none may be 0 --------

d="$(tree fetchfail "9")"
entry 9 'A computed const is rejected' >>"$d/LIMITATIONS.md"

broken="$work/broken.sh"
printf '#!/usr/bin/env bash\necho "curl: (6) could not resolve host" >&2\nexit 1\n' >"$broken"
chmod +x "$broken"
expect "the fetcher errors on list" "$d" "$broken" 2 "cannot answer"

# The dangerous one. A fetcher that succeeds and says nothing looks exactly like
# "no issue is labelled", which would be a clean pass on an empty roster -- so
# the roster from the criteria document is what turns it into a `get`, and the
# `get` failing is what makes it exit 2.
silent="$work/silent.sh"
printf '#!/usr/bin/env bash\ncase "$1" in list) exit 0 ;; *) exit 1 ;; esac\n' >"$silent"
chmod +x "$silent"
expect "the fetcher answers nothing at all" "$d" "$silent" 2 "asking GitHub about issue #9 failed"

# An empty label answer *and* an empty roster is not a pass either: a run that
# examined nothing must say it could not answer, not that the criterion is met.
d="$(tree noroster "")"
expect "nothing labelled and nothing named" "$d" "$silent" 2 "found no issues labelled"

# A fetcher that answers `list` and then fails on `get` -- a rate limit reached
# partway through.
d="$(tree ratelimit "9 10")"
entry 9 'A computed const is rejected' >>"$d/LIMITATIONS.md"
entry 10 'A field access needs a type' >>"$d/LIMITATIONS.md"
half="$work/half.sh"
{
  echo '#!/usr/bin/env bash'
  echo 'case "$1" in'
  printf '  list) printf "9\\tclosed\\tcompleted\\tv1.0-limitation\\tcomputed const\\n" ;;\n'
  echo '  *) echo "GitHub answered 403 -- a rate limit" >&2; exit 1 ;;'
  echo 'esac'
} >"$half"
chmod +x "$half"
expect "a rate limit partway through" "$d" "$half" 2 "cannot answer"

# --- the documents have to be there at all ----------------------------------

f="$(stub trivial "$(row 9 closed completed v1.0-limitation 'computed const')")"

d="$work/nodoc"
mkdir -p "$d"
printf '## 4. x\n\n#9\n' >"$d/RELEASE-CRITERIA-1.0.md"
expect "no LIMITATIONS.md" "$d" "$f" 2 "LIMITATIONS.md does not exist"

d="$work/nocriteria"
mkdir -p "$d"
printf '# What W# does not do\n' >"$d/LIMITATIONS.md"
expect "no RELEASE-CRITERIA-1.0.md" "$d" "$f" 2 "RELEASE-CRITERIA-1.0.md does not exist"

expect "a tree that is not there" "$work/absent" "$f" 2 "cannot read the tree"

# The live gate runs separately in CI with its token. Fixture selftests are
# offline; failed live fetches must fail that gate, never skip it.
echo
if [ "$failures" -gt 0 ]; then
  echo "$failures of $checks checks failed."
  exit 1
fi
echo "$checks checks passed."
