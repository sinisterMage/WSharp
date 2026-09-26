#!/usr/bin/env bash
#
# Criterion 7: a clean-machine install, actually performed.
#
#   scripts/verify-install.sh <version> <triple> [--tool wsharp|sharpie]
#
# The only install that counts is one on a machine with no prior W#, no cached
# toolchain and no developer environment. Reading the release workflow is not
# verification, so this script does what a stranger does and checks the parts a
# stranger would not notice: that the digest was verified *before* anything was
# extracted, that nothing landed outside the documented prefix, that the shell
# profile was left alone, and that a damaged download is refused with a message
# rather than extracted.
#
# It is deliberately dependency-free beyond `curl`, `tar`, `sha256sum`/`shasum`
# and `find`: a script that needed a package manager could not run on the clean
# image it is supposed to be testing.
#
# Exit 0 means every check below passed. Any other exit is a failed gate, and
# the last line of output names which one.

set -uo pipefail

readonly TRIPLES=(
  x86_64-unknown-linux-gnu
  x86_64-pc-windows-msvc
  x86_64-apple-darwin
  aarch64-apple-darwin
)

usage() {
  cat >&2 <<EOF
usage: scripts/verify-install.sh <version> <triple> [--tool wsharp|sharpie]

  <version>  release version without a leading v, e.g. 0.2.3
  <triple>   one of: ${TRIPLES[*]}
  --tool     which artefact to verify; default wsharp

  --keep     leave the scratch directory behind for inspection
EOF
  exit 2
}

# ---------------------------------------------------------------------------
# Reporting. Every check prints one line beginning "ok" or "FAIL", so the
# transcript pasted into a task comment is readable without the script.
# ---------------------------------------------------------------------------

failures=0
checks=0

ok()   { checks=$((checks + 1)); echo "ok   $*"; }
fail() { checks=$((checks + 1)); failures=$((failures + 1)); echo "FAIL $*"; }
info() { echo "     $*"; }
die()  { echo "FATAL $*" >&2; exit 3; }

# ---------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------

[ $# -ge 2 ] || usage
VERSION="$1"; shift
TRIPLE="$1"; shift
TOOL="wsharp"
KEEP=0

while [ $# -gt 0 ]; do
  case "$1" in
    --tool) shift; [ $# -gt 0 ] || usage; TOOL="$1" ;;
    --tool=*) TOOL="${1#--tool=}" ;;
    --keep) KEEP=1 ;;
    *) usage ;;
  esac
  shift
done

case "$TOOL" in
  wsharp|sharpie) ;;
  *) echo "error: --tool must be wsharp or sharpie, not '$TOOL'" >&2; usage ;;
esac

triple_is_known=0
for t in "${TRIPLES[@]}"; do [ "$TRIPLE" = "$t" ] && triple_is_known=1; done
if [ "$triple_is_known" -eq 0 ]; then
  echo "error: '$TRIPLE' is not one of the four release triples." >&2
  echo "       A platform claimed by inference from another platform's run is" >&2
  echo "       not evidence, so this script refuses a triple it cannot name." >&2
  usage
fi

case "$TOOL" in
  wsharp)  REPO="sinisterMage/WSharp" ;;
  sharpie) REPO="sinisterMage/sharpie" ;;
esac

readonly ASSET="${TOOL}-${VERSION}-${TRIPLE}.tar.gz"
readonly BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"

# ---------------------------------------------------------------------------
# A digest tool. macOS has `shasum -a 256` and not `sha256sum`; Linux images
# vary. Resolve once rather than at every call site.
# ---------------------------------------------------------------------------

if command -v sha256sum >/dev/null 2>&1; then
  digest_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  digest_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "no sha256sum and no shasum: cannot verify a digest, so cannot verify an install"
fi

command -v curl >/dev/null 2>&1 || die "no curl"
command -v tar  >/dev/null 2>&1 || die "no tar"

# ---------------------------------------------------------------------------
# Scratch layout. PREFIX is the documented install prefix; everything else is
# the harness's own and is excluded from the "nothing outside the prefix"
# manifest diff by being inside WORK, which is itself excluded.
# ---------------------------------------------------------------------------

WORK="$(mktemp -d "${TMPDIR:-/tmp}/verify-install.XXXXXX")" || die "mktemp failed"
readonly WORK
readonly PREFIX="$WORK/prefix"
readonly DOWNLOADS="$WORK/downloads"
mkdir -p "$PREFIX" "$DOWNLOADS"

cleanup() {
  if [ "$KEEP" -eq 1 ]; then
    echo "     scratch kept at $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

echo "verify-install: $TOOL $VERSION $TRIPLE"
echo "     repo   $REPO"
echo "     asset  $ASSET"
echo "     prefix $PREFIX"
echo

# ---------------------------------------------------------------------------
# 1. Download the tarball and its published digest.
# ---------------------------------------------------------------------------

TARBALL="$DOWNLOADS/$ASSET"
SIDECAR="$DOWNLOADS/$ASSET.sha256"

# `--fail` so a 404 is an error rather than a file containing GitHub's HTML,
# which would otherwise fail the digest check and report the wrong cause.
if curl -sSfL -o "$TARBALL" "$BASE_URL/$ASSET" 2>"$WORK/curl.err"; then
  ok "1. downloaded $ASSET ($(wc -c <"$TARBALL" | tr -d ' ') bytes)"
else
  fail "1. could not download $BASE_URL/$ASSET"
  info "$(head -2 "$WORK/curl.err")"
  echo; echo "$checks checks, $failures failed"; exit 1
fi

if curl -sSfL -o "$SIDECAR" "$BASE_URL/$ASSET.sha256" 2>"$WORK/curl.err"; then
  ok "1. downloaded the published digest sidecar"
else
  fail "1. no published digest sidecar at $BASE_URL/$ASSET.sha256"
  info "1.0 ships digest sidecars; a release without one cannot be verified."
  echo; echo "$checks checks, $failures failed"; exit 1
fi

# The sidecar is `<digest>  <filename>`. Take the first field only, so a
# sidecar naming a path rather than a bare name still compares.
PUBLISHED="$(awk 'NR==1 {print $1}' "$SIDECAR")"
OBSERVED="$(digest_of "$TARBALL")"

echo "     digest published  $PUBLISHED"
echo "     digest observed   $OBSERVED"

# ---------------------------------------------------------------------------
# 2. Verify the digest BEFORE extracting anything.
#
# The ordering is the check. A script that extracted first and compared second
# would pass this file's every other assertion while giving a tampered archive
# the chance to write wherever it liked.
# ---------------------------------------------------------------------------

if [ -z "$PUBLISHED" ]; then
  fail "2. the sidecar carried no digest"
elif [ "$OBSERVED" = "$PUBLISHED" ]; then
  ok "2. digest verified before extracting"
else
  fail "2. digest mismatch: refusing to extract"
  echo; echo "$checks checks, $failures failed"; exit 1
fi

# Nothing has been extracted at this point; assert that, so the ordering is
# checked rather than merely intended.
if [ -z "$(ls -A "$PREFIX")" ]; then
  ok "2. nothing was extracted before the digest was verified"
else
  fail "2. the prefix already held files before extraction"
fi

# ---------------------------------------------------------------------------
# 5a. Filesystem manifest, before. Taken now, immediately before the only step
# that writes anything outside WORK, so that anything the install creates is
# attributable to the install.
#
# Scoped to the directories an installer plausibly touches. A whole-filesystem
# manifest would be dominated by unrelated churn -- logs, caches, /proc -- and
# a check that is noisy is a check that gets ignored.
# ---------------------------------------------------------------------------

MANIFEST_ROOTS=("$HOME")
for extra in /usr/local/bin /usr/local/lib /opt; do
  [ -d "$extra" ] && MANIFEST_ROOTS+=("$extra")
done

take_manifest() {
  # Exclude WORK (the harness's own scratch) and the noisiest caches. `-prune`
  # rather than a grep, so a large cache is not walked at all.
  find "${MANIFEST_ROOTS[@]}" \
    \( -path "$WORK" -o -name '.cache' -o -name '.npm' -o -name '.git' \) -prune \
    -o -print 2>/dev/null | LC_ALL=C sort
}

take_manifest > "$WORK/manifest.before"
ok "5. filesystem manifest taken before the install ($(wc -l <"$WORK/manifest.before" | tr -d ' ') paths)"

# ---------------------------------------------------------------------------
# 6a. The shell profile, before.
# ---------------------------------------------------------------------------

readonly PROFILES=(
  "$HOME/.profile" "$HOME/.bash_profile" "$HOME/.bashrc"
  "$HOME/.zshrc" "$HOME/.zshenv" "$HOME/.config/fish/config.fish"
)

profile_state() {
  local p
  for p in "${PROFILES[@]}"; do
    if [ -f "$p" ]; then echo "$p $(digest_of "$p")"; else echo "$p absent"; fi
  done
}

profile_state > "$WORK/profiles.before"

# ---------------------------------------------------------------------------
# 3. Extract into the documented prefix and nowhere else.
#
# Checked rather than assumed: `tar -t` is read first and every member is
# required to be a relative path under a single top-level directory. An archive
# with an absolute path or a `..` component would escape the prefix, and GNU tar
# strips the leading slash with a warning that is easy to miss.
# ---------------------------------------------------------------------------

escapes=0
while IFS= read -r member; do
  case "$member" in
    /*)    escapes=$((escapes + 1)); info "absolute path in archive: $member" ;;
    *../*) escapes=$((escapes + 1)); info "parent reference in archive: $member" ;;
    ../*)  escapes=$((escapes + 1)); info "parent reference in archive: $member" ;;
  esac
done < <(tar tzf "$TARBALL")

if [ "$escapes" -eq 0 ]; then
  ok "3. every archive member is a relative path under the prefix"
else
  fail "3. the archive carries $escapes path(s) that would escape the prefix"
fi

if tar xzf "$TARBALL" -C "$PREFIX" 2>"$WORK/tar.err"; then
  ok "3. extracted into the prefix"
else
  fail "3. extraction failed"
  info "$(head -3 "$WORK/tar.err")"
fi

# The archives unpack into a single `<tool>-<version>-<triple>/` directory.
ROOT="$PREFIX/${TOOL}-${VERSION}-${TRIPLE}"
[ -d "$ROOT" ] || ROOT="$PREFIX"

# ---------------------------------------------------------------------------
# 4. Run a hello program through `wsharp run` and again through `wsharp build`,
#    and run `ingot help`.
#
# Only meaningful for the W# artefact; sharpie's own rungs are `tests/rungs.sh`
# in its repository, which is the other half of this criterion.
# ---------------------------------------------------------------------------

if [ "$TOOL" = "wsharp" ]; then
  WSHARP="$ROOT/wsharp"
  INGOT="$ROOT/ingot"

  # Windows artefacts carry `.exe`; the same script must verify that triple.
  [ -x "$WSHARP" ] || [ -f "$WSHARP" ] || WSHARP="$ROOT/wsharp.exe"
  [ -x "$INGOT" ]  || [ -f "$INGOT" ]  || INGOT="$ROOT/ingot.exe"

  if [ -f "$WSHARP" ]; then
    ok "4. the prefix holds a wsharp binary"
  else
    fail "4. no wsharp binary under $ROOT"
  fi

  HELLO="$WORK/hello.ws"
  # `void` stated: inference rejects a function whose body can fall through
  # without returning the type it claims, so a bare `fn main()` is a compile
  # error rather than a hello program.
  printf 'fn main() void {\n    print("hello from a clean install");\n}\n' > "$HELLO"
  readonly EXPECTED="hello from a clean install"

  if out="$("$WSHARP" run "$HELLO" 2>&1)"; then
    if [ "$out" = "$EXPECTED" ]; then
      ok "4. wsharp run printed what the program prints"
    else
      fail "4. wsharp run printed '$out', wanted '$EXPECTED'"
    fi
  else
    fail "4. wsharp run exited non-zero"
    info "$(echo "$out" | head -3)"
  fi

  # `wsharp build` links with a C compiler, which a clean image need not have.
  # That is a real precondition of the gate rather than a reason to skip it, so
  # it is reported as a failure naming the missing tool -- an install that
  # cannot build is an install that half works, and the release notes have to
  # say so either way.
  if command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 \
     || command -v clang >/dev/null 2>&1 || [ -n "${CC:-}" ]; then
    if out="$("$WSHARP" build "$HELLO" -o "$WORK/hello" 2>&1)"; then
      if built="$("$WORK/hello" 2>&1)" && [ "$built" = "$EXPECTED" ]; then
        ok "4. wsharp build produced an executable that printed the same thing"
      else
        fail "4. the built executable printed '$built', wanted '$EXPECTED'"
      fi
    else
      fail "4. wsharp build exited non-zero"
      info "$(echo "$out" | head -3)"
    fi
  else
    fail "4. no C compiler on PATH, so wsharp build cannot be exercised"
    info "wsharp build links with cc. A clean image must provide one, and the"
    info "install documentation must say so. Install a C compiler and re-run."
  fi

  if [ -f "$INGOT" ]; then
    if out="$("$INGOT" help 2>&1)"; then
      ok "4. ingot help exited 0"
    else
      fail "4. ingot help exited non-zero"
      info "$(echo "$out" | head -3)"
    fi
  else
    fail "4. no ingot binary under $ROOT"
  fi
else
  if [ -f "$ROOT/sharpie" ] || [ -f "$ROOT/sharpie.exe" ]; then
    ok "4. the prefix holds a sharpie binary"
  else
    fail "4. no sharpie binary under $ROOT"
  fi
  info "4. sharpie's resolution rungs are tests/rungs.sh in sinisterMage/sharpie."
fi

# ---------------------------------------------------------------------------
# 5b. Nothing was created outside the prefix.
# ---------------------------------------------------------------------------

take_manifest > "$WORK/manifest.after"

if new_paths="$(comm -13 "$WORK/manifest.before" "$WORK/manifest.after")" \
   && [ -z "$new_paths" ]; then
  ok "5. nothing was created outside the prefix"
else
  fail "5. the install created $(echo "$new_paths" | grep -c .) path(s) outside the prefix"
  echo "$new_paths" | head -20 | while IFS= read -r p; do info "$p"; done
fi

# ---------------------------------------------------------------------------
# 6b. The shell profile was not edited.
# ---------------------------------------------------------------------------

profile_state > "$WORK/profiles.after"

if diff -q "$WORK/profiles.before" "$WORK/profiles.after" >/dev/null 2>&1; then
  ok "6. the shell profile was not edited"
else
  fail "6. a shell profile changed during the install"
  diff "$WORK/profiles.before" "$WORK/profiles.after" | head -10 \
    | while IFS= read -r l; do info "$l"; done
fi

# ---------------------------------------------------------------------------
# 7. Repeat 1-3 with a truncated download and with a corrupted one, and require
#    a refusal with a message in both cases.
#
# Both are run through the same verify-and-extract path the real install uses,
# so what is being tested is the decision rather than a restatement of it. The
# assertion has two halves and both matter: the archive must not be extracted,
# *and* the refusal must say something. A silent refusal is a bug report that
# never gets filed.
# ---------------------------------------------------------------------------

# Verify a candidate against the published digest exactly as step 2 does, and
# print a refusal. Extracts only on a match, into its own empty directory.
verify_and_extract() {
  local candidate="$1" dest="$2" observed
  observed="$(digest_of "$candidate")"
  if [ "$observed" != "$PUBLISHED" ]; then
    echo "refusing: digest mismatch for $(basename "$candidate")"
    echo "  published $PUBLISHED"
    echo "  observed  $observed"
    return 1
  fi
  mkdir -p "$dest"
  tar xzf "$candidate" -C "$dest"
}

check_damaged() {
  local name="$1" candidate="$2" dest="$WORK/damaged-$1"
  local out status
  mkdir -p "$dest"
  out="$(verify_and_extract "$candidate" "$dest" 2>&1)"
  status=$?

  if [ "$status" -eq 0 ]; then
    fail "7. a $name download was accepted"
    return
  fi
  if [ -z "$(ls -A "$dest")" ]; then
    ok "7. a $name download was refused and nothing was extracted"
  else
    fail "7. a $name download was refused but something was extracted anyway"
  fi
  if [ -n "$out" ]; then
    ok "7. the refusal of a $name download carried a message"
    info "$(echo "$out" | head -1)"
  else
    fail "7. a $name download was refused silently"
  fi
}

# Truncated: the first 90% of the bytes. A prefix of a gzip stream is the
# realistic failure -- an interrupted transfer -- and it must be caught by the
# digest rather than by tar noticing later.
TRUNC="$DOWNLOADS/truncated.tar.gz"
full_size="$(wc -c <"$TARBALL" | tr -d ' ')"
head -c "$(( full_size * 9 / 10 ))" "$TARBALL" > "$TRUNC"
check_damaged "truncated" "$TRUNC"

# Corrupted: the same length, one byte different. Same length on purpose -- a
# check that only compared sizes would pass the truncated case and fail here,
# so the pair distinguishes a digest check from a length check.
CORRUPT="$DOWNLOADS/corrupted.tar.gz"
cp "$TARBALL" "$CORRUPT"
# Flip a byte in the middle of the compressed stream.
printf '\xff' | dd of="$CORRUPT" bs=1 seek="$(( full_size / 2 ))" count=1 \
  conv=notrunc status=none 2>/dev/null
if [ "$(digest_of "$CORRUPT")" = "$PUBLISHED" ]; then
  # A one-byte flip that lands on the value already there changes nothing.
  printf '\x00' | dd of="$CORRUPT" bs=1 seek="$(( full_size / 2 ))" count=1 \
    conv=notrunc status=none 2>/dev/null
fi
if [ "$(wc -c <"$CORRUPT" | tr -d ' ')" = "$full_size" ]; then
  ok "7. the corrupted candidate is the same length as the real one"
else
  fail "7. the corrupted candidate changed length, so this is a length test"
fi
check_damaged "corrupted" "$CORRUPT"

# ---------------------------------------------------------------------------

echo
echo "$TOOL $VERSION $TRIPLE"
echo "  digest observed   $OBSERVED"
echo "  digest published  $PUBLISHED"
if [ "$failures" -eq 0 ]; then
  echo "$checks checks, all passed"
  exit 0
fi
echo "$checks checks, $failures failed"
exit 1
