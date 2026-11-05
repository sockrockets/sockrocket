#!/usr/bin/env bash
# CI packaging entry point. Verify the actual ARM64 allocator before stripping.
set -euo pipefail
cd "$(dirname "$0")/.."
target=${1:?expected a Merlin Rust target}
case "$target" in
  aarch64-unknown-linux-musl)
    strip_tool=${MERLIN_STRIP_TOOL:-llvm-strip}
    command -v "$strip_tool" >/dev/null
    cross rustc --locked --release --package sockrocket-cli --target "$target" -- -C strip=none
    binary="${CARGO_TARGET_DIR:-target}/$target/release/sockrocket-cli"
    python3 tools/verify-merlin-allocator.py "$binary"
    "$strip_tool" --strip-all "$binary"
    ;;
  armv7-unknown-linux-musleabihf|armv7-unknown-linux-musleabi)
    cross build --locked --release --package sockrocket-cli --target "$target"
    ;;
  *) echo 'Unsupported Merlin target' >&2; exit 2 ;;
esac
