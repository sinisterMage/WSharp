#!/usr/bin/env bash
# Build the Joe Linux compiler and the archive used by W# executables.
set -euo pipefail
bootstrap=$(cd "${1:?usage: scripts/build-joe.sh /path/to/joe-linux-aarch64}/build/bootstrap" && pwd)
cd "$(dirname "$0")/.."
export RUSTC="$bootstrap/rust/bin/rustc"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bootstrap/root/usr/lib/joe-cross/bin/clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export AR_aarch64_linux_android="$bootstrap/root/usr/lib/joe-cross/bin/llvm-ar"
exec "$bootstrap/rust/bin/cargo" build --locked --release --target aarch64-linux-android \
    --workspace --features wsharp-runtime/joe-linux
