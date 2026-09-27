#!/usr/bin/env bash
# Offline fault injection through the real verifier; not clean-machine evidence.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${VERIFY_INSTALL_SCRIPT:-$ROOT/scripts/verify-install.sh}"
TEST="$(mktemp -d "${TMPDIR:-/tmp}/verify-inspection.XXXXXX")"
trap 'rm -rf "$TEST"' EXIT
export FIXTURE="$TEST" REAL_FIND="$(command -v find)" REAL_TAR="$(command -v tar)"
export REAL_SORT="$(command -v sort)" REAL_COMM="$(command -v comm)"
mkdir -p "$TEST/bin" "$TEST/archive" "$TEST/home" "$TEST/tmp"
# Pre-existing directories ensure only the forbidden file is new.
mkdir -p "$TEST/home/.cache" "$TEST/home/.npm" "$TEST/home/project/.git"
printf 'fixture only\n' > "$TEST/archive/sharpie"
tar czf "$TEST/asset.tar.gz" -C "$TEST/archive" sharpie
if command -v sha256sum >/dev/null; then
  sha256sum "$TEST/asset.tar.gz" > "$TEST/asset.sha256"
else
  shasum -a 256 "$TEST/asset.tar.gz" > "$TEST/asset.sha256"
fi
cat > "$TEST/bin/curl" <<'SH'
#!/usr/bin/env bash
[ "$FAULT" != fetch ] || { echo 'injected fetch/auth/network failure' >&2; exit 22; }
while [ "$1" != -o ]; do shift; done
case "$3" in *.sha256) cp "$FIXTURE/asset.sha256" "$2";; *) cp "$FIXTURE/asset.tar.gz" "$2";; esac
SH
cat > "$TEST/bin/find" <<'SH'
#!/usr/bin/env bash
n=0
[ ! -f "$FIXTURE/count" ] || read -r n < "$FIXTURE/count"
n=$((n + 1)); echo "$n" > "$FIXTURE/count"
case "$FAULT:$n" in
  marker:1|before:2|after:3) echo 'injected inspection failure' >&2; exit 1;;
esac
exec "$REAL_FIND" "$@"
SH
cat > "$TEST/bin/tar" <<'SH'
#!/usr/bin/env bash
if [ "$1" = tzf ]; then
  case "$FAULT" in
    archive-list) printf 'sharpie\n'; echo 'injected partial archive inspection' >&2; exit 2;;
    archive-path) printf '../escape\n'; exit 0;;
  esac
fi
if [ "$1" = xzf ]; then : > "$FIXTURE/extracted"; fi
if [ "$FAULT" = leak ] && [ "$1" = xzf ]; then
  : > "$HOME/outside-prefix"
fi
if [ "$1" = xzf ]; then
  case "$FAULT" in
    host-write)
      # A sibling of the verifier writes strictly between manifest snapshots.
      printf 'write\n' > "$FIXTURE/request"
      read -r acknowledged < "$FIXTURE/ack"
      [ "$acknowledged" = written ] || exit 4;;
    transient-leak)
      # A child writes through an absolute path, then removes the file.
      bash -c ': > "$1"; rm "$1"' _ "$HOME/outside-prefix";;
    cache-leak) : > "$HOME/.cache/outside-prefix";;
    npm-leak) : > "$HOME/.npm/outside-prefix";;
    git-leak) : > "$HOME/project/.git/outside-prefix";;
  esac
fi
exec "$REAL_TAR" "$@"
SH
for tool in sort comm; do
  cat > "$TEST/bin/$tool" <<'SH'
#!/usr/bin/env bash
tool="${0##*/}"
[ "$FAULT" != "$tool" ] || { echo "injected $tool failure" >&2; exit 1; }
case "$tool" in sort) exec "$REAL_SORT" "$@";; comm) exec "$REAL_COMM" "$@";; esac
SH
done
chmod +x "$TEST/bin/"*
failures=0
faults=(clean leak cache-leak npm-leak git-leak marker before after sort comm fetch archive-list archive-path)
# Opt-in acceptance probe: intentionally red until attribution is implemented.
if [ "${VERIFY_INSTALL_ATTRIBUTION_PROBE:-0}" = 1 ]; then
  faults+=(host-write transient-leak)
fi
for fault in "${faults[@]}"; do
  rm -f "$TEST/count" "$TEST/home/outside-prefix" "$TEST/extracted" \
    "$TEST/home/.cache/outside-prefix" "$TEST/home/.npm/outside-prefix" \
    "$TEST/home/project/.git/outside-prefix"
  writer_pid=
  if [ "$fault" = host-write ]; then
    mkfifo "$TEST/request" "$TEST/ack"
    (
      read -r request < "$TEST/request"
      [ "$request" = write ] || exit 4
      : > "$TEST/home/outside-prefix"
      printf 'written\n' > "$TEST/ack"
    ) &
    writer_pid=$!
  fi
  status=0
  env HOME="$TEST/home" TMPDIR="$TEST/tmp" PATH="$TEST/bin:$PATH" FAULT="$fault" \
    bash "$SCRIPT" 0.1.2 x86_64-unknown-linux-gnu --tool sharpie > "$TEST/out" 2>&1 || status=$?
  if [ -n "$writer_pid" ]; then
    # Do not leave the sibling waiting if extraction was never reached.
    kill "$writer_pid" 2>/dev/null || true
    wait "$writer_pid" 2>/dev/null || true
    rm "$TEST/request" "$TEST/ack"
  fi
  case "$fault" in
    host-write) pattern='checks, all passed'; expected=0;;
    transient-leak) pattern='outside the prefix'; expected=1;;
    clean) pattern='checks, all passed'; expected=0;;
    leak|cache-leak|npm-leak|git-leak) pattern='path(s) outside the prefix'; expected=1;;
    marker) pattern='FATAL 5. cannot discover scratch aliases'; expected=3;;
    before) pattern='FATAL 5. cannot inspect filesystem before install'; expected=3;;
    after) pattern='FATAL 5. cannot inspect filesystem after install'; expected=3;;
    sort) pattern='FATAL 5. cannot inspect filesystem before install'; expected=3;;
    comm) pattern='FATAL 5. cannot compare filesystem manifests'; expected=3;;
    archive-list) pattern='FATAL 3. cannot inspect archive members'; expected=3;;
    archive-path) pattern='FATAL 3. refusing to extract'; expected=3;;
    fetch) pattern='FAIL 1. could not download'; expected=1;;
  esac
  case "$fault" in
    archive-list|archive-path)
      if [ -f "$TEST/extracted" ]; then
        echo "FAIL $fault: extraction attempted after rejected inspection"
        failures=$((failures + 1))
      fi;;
  esac
  if [ "$status" -eq "$expected" ] && grep -Fq "$pattern" "$TEST/out"; then
    echo "ok $fault (exit $status)"
  else
    echo "FAIL $fault (exit $status, expected $expected / $pattern)"
    cat "$TEST/out"
    failures=$((failures + 1))
  fi
done
[ "$failures" -eq 0 ]
