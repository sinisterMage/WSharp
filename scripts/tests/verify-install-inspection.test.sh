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
if [ "$FAULT" = leak ] && [ "$1" = xzf ]; then
  : > "$HOME/outside-prefix"
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
for fault in clean leak marker before after sort comm fetch; do
  rm -f "$TEST/count" "$TEST/home/outside-prefix"
  status=0
  env HOME="$TEST/home" TMPDIR="$TEST/tmp" PATH="$TEST/bin:$PATH" FAULT="$fault" \
    bash "$SCRIPT" 0.1.2 x86_64-unknown-linux-gnu --tool sharpie > "$TEST/out" 2>&1 || status=$?
  case "$fault" in
    clean) pattern='checks, all passed'; expected=0;;
    leak) pattern='path(s) outside the prefix'; expected=1;;
    marker) pattern='FATAL 5. cannot discover scratch aliases'; expected=3;;
    before) pattern='FATAL 5. cannot inspect filesystem before install'; expected=3;;
    after) pattern='FATAL 5. cannot inspect filesystem after install'; expected=3;;
    sort) pattern='FATAL 5. cannot inspect filesystem before install'; expected=3;;
    comm) pattern='FATAL 5. cannot compare filesystem manifests'; expected=3;;
    fetch) pattern='FAIL 1. could not download'; expected=1;;
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
