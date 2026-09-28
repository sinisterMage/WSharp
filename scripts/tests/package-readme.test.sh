#!/usr/bin/env bash
# Execute the first Packages recipe verbatim with a fresh, offline store.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
doc=${1:-$root/README.md}
ingot_bin=$(command -v "${INGOT:-ingot}")
wsharp_bin=$(command -v "${WSHARP:-wsharp}")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/registry" "$work/project" "$work/bin"
printf '[registry]\nversion = 1\nname = "offline-doc-test"\n' > "$work/registry/Registry.toml"
ln -s "$ingot_bin" "$work/bin/ingot"
ln -s "$wsharp_bin" "$work/bin/wsharp"
export PATH="$work/bin:$PATH" WSHARP_HOME="$work/store" INGOT_REGISTRY="$work/registry"
awk '
  /^## (Packages|Local example)$/ || /^title: "Packages"$/ { packages=1; next }
  packages && /^```sh$/ { block=1; next }
  block && /^```$/ { exit }
  block { print }
' "$doc" > "$work/recipe.sh"
[ -s "$work/recipe.sh" ] || { echo 'missing Packages recipe' >&2; exit 1; }
(cd "$work/project" && bash -eu "$work/recipe.sh")
ingot -C "$work/project/myapp" verify
[ "$(wsharp run "$work/project/myapp/src/myapp.ws")" = 42 ]
[ -s "$work/project/myapp/ingot.lock" ]
[ -s "$work/project/myapp/ingot.env" ]
echo 'PASS: README package recipe resolves, installs, verifies and prints 42 offline'
