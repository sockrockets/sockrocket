#!/usr/bin/env bash
# CI packaging entry point. Verify the actual ARM64 allocator before stripping.
set -euo pipefail
cd "$(dirname "$0")/.."
target=${1:?expected a Merlin Rust target}
case "$target" in
  aarch64-unknown-linux-musl)
    # The hosted Merlin builders use x86_64 Linux. Match the C compiler used
    # for the size and forwarding measurements on the local ARM64 build.
    if [[ "$(zig version 2>/dev/null || true)" != 0.16.0 ]]; then
      if [[ "${GITHUB_ACTIONS:-}" != true || "$(uname -sm)" != "Linux x86_64" ]]; then
        echo 'Install Zig 0.16.0 to build the Merlin ARM64 package.' >&2
        exit 1
      fi
      tool_dir=$(mktemp -d "${RUNNER_TEMP}/merlin-zig.XXXXXXXX")
      trap 'rm -rf "$tool_dir"' EXIT
      archive="$tool_dir/zig.tar.xz"
      curl --fail --location --retry 3 --output "$archive" https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz
      echo "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00  $archive" | sha256sum --check
      tar -xJf "$archive" -C "$tool_dir"
      export PATH="$tool_dir/zig-x86_64-linux-0.16.0:$PATH"
    fi
    CARGO_NET_OFFLINE=false bash tools/build-merlin-musl.sh --locked
    ;;
  armv7-unknown-linux-musleabihf|armv7-unknown-linux-musleabi)
    cross build --locked --release --package sockrocket-cli --target "$target"
    ;;
  *) echo 'Unsupported Merlin target' >&2; exit 2 ;;
esac
