#!/bin/bash
# Compile C dependencies with Zig, but use Rust's bundled musl for final linking.
# Zig 0.16's C runtime supplies SmpAllocator-backed malloc/free, which retain
# small allocation slabs after free; see the allocator retention investigation.
# Canonical build instructions: docs/merlin-local-build.md.
set -euo pipefail
cd "$(dirname "$0")/.."
toolchain_root=$(rustc --print sysroot)
host_triple=$(rustc -vV | awk '/^host:/ {print $2}')
rust_lld="$toolchain_root/lib/rustlib/$host_triple/bin/rust-lld"
test -x "$rust_lld"
test -f "$toolchain_root/lib/rustlib/aarch64-unknown-linux-musl/lib/self-contained/libc.a"
if [ -n "${MERLIN_STRIP_TOOL:-}" ]; then
  strip_tool="$MERLIN_STRIP_TOOL"
elif command -v llvm-strip >/dev/null 2>&1; then
  strip_tool=$(command -v llvm-strip)
elif [ -x /opt/homebrew/opt/llvm@21/bin/llvm-strip ]; then
  strip_tool=/opt/homebrew/opt/llvm@21/bin/llvm-strip
elif compgen -G '/usr/lib/llvm-*/bin/llvm-strip' >/dev/null; then
  strip_tools=(/usr/lib/llvm-*/bin/llvm-strip)
  strip_tool=${strip_tools[0]}
else
  strip_tool=$(command -v aarch64-linux-musl-strip) || {
    echo 'Set MERLIN_STRIP_TOOL to an ELF-capable llvm-strip or aarch64-linux-musl-strip.' >&2
    exit 1
  }
fi
test -x "$strip_tool"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$rust_lld"
export CC_aarch64_unknown_linux_musl="$PWD/tools/zig-cc-aarch64-musl"
export AR_aarch64_unknown_linux_musl="$PWD/tools/zig-ar-aarch64-musl"
source tools/merlin-arm64-rustc-flags.sh
# Local builds default to offline. CI fetches locked target and host build
# dependencies as needed, including procedural macro dependencies on the host.
CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-true}" cargo rustc "${merlin_arm64_cargo_flags[@]}" -p sockrocket-cli --release --target aarch64-unknown-linux-musl "$@" -- "${merlin_arm64_flags[@]}"
binary="${CARGO_TARGET_DIR:-$PWD/target}/aarch64-unknown-linux-musl/release/sockrocket-cli"
python3 tools/verify-merlin-allocator.py "$binary"
cp "$binary" "$binary.symbols"
"$strip_tool" --strip-all "$binary"
bytes=$(wc -c < "$binary")
max_bytes=11100000
if (( bytes > max_bytes )); then
  echo "Merlin ARM64 CLI is $bytes bytes; the package limit is $max_bytes." >&2
  exit 1
fi
