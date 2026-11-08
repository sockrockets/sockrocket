#!/bin/bash
# Assemble Merlin / koolcenter offline packages for all fancyss-style platforms.
#
# Platform map (same convention as hq450/fancyss):
#   arm      -> armv7 softfloat-ish routers (BCM470x); binary: armv7 musleabi
#   hnd      -> Broadcom 32-bit (BCM675x);            binary: armv7 musleabihf
#   qca      -> Qualcomm IPQ807x;                      binary: armv7 musleabihf
#   ipq32    -> Qualcomm IPQ53xx 32-bit;               binary: armv7 musleabihf
#   hnd_v8   -> Broadcom 64-bit (BCM490x);             binary: aarch64 musl
#   mtk      -> MediaTek MT798x;                       binary: aarch64 musl
#   ipq64    -> Qualcomm IPQ53xx 64-bit;               binary: aarch64 musl
#
# Usage:
#   For local ARM64 builds, first run bash tools/build-merlin-musl.sh.
#   See docs/merlin-local-build.md for allocator checks. This script copies
#   binaries as supplied; it does not verify or change their allocator.
#   ./merlin/pack.sh
#     Expects release binaries already built at:
#       target/aarch64-unknown-linux-musl/release/sockrocket-cli
#       target/armv7-unknown-linux-musleabihf/release/sockrocket-cli
#       target/armv7-unknown-linux-musleabi/release/sockrocket-cli
#
#   OUT_DIR=dist/merlin ./merlin/pack.sh

set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT/dist/merlin}"
mkdir -p "$OUT_DIR"

AARCH64_BIN="${AARCH64_BIN:-$ROOT/target/aarch64-unknown-linux-musl/release/sockrocket-cli}"
ARMV7HF_BIN="${ARMV7HF_BIN:-$ROOT/target/armv7-unknown-linux-musleabihf/release/sockrocket-cli}"
ARMV7SF_BIN="${ARMV7SF_BIN:-$ROOT/target/armv7-unknown-linux-musleabi/release/sockrocket-cli}"

if [ ! -x "$AARCH64_BIN" ]; then
  echo "missing aarch64 binary: $AARCH64_BIN" >&2
  exit 1
fi
if [ ! -x "$ARMV7HF_BIN" ]; then
  echo "missing armv7hf binary: $ARMV7HF_BIN" >&2
  exit 1
fi
if [ ! -x "$ARMV7SF_BIN" ]; then
  echo "missing armv7 softfloat binary: $ARMV7SF_BIN" >&2
  exit 1
fi

python3 "$ROOT/tools/verify-merlin-targets.py" "$AARCH64_BIN" "$ARMV7HF_BIN" "$ARMV7SF_BIN"

pack_one() {
  local platform="$1"
  local src_bin="$2"
  local work
  work="$(mktemp -d)"
  local pkg="$work/sockrocket"

  mkdir -p "$pkg/bin" "$pkg/webui" "$pkg/scripts" "$pkg/webs" "$pkg/res"

  # Both the bundled installer and online updater use this canonical path.
  # Do not duplicate the binary under an architecture suffix or add links:
  # online updates deliberately accept only regular files and directories.
  cp "$src_bin" "$pkg/bin/sockrocket-cli"
  chmod +x "$pkg/bin/sockrocket-cli"

  cp "$ROOT/merlin/install.sh" "$ROOT/merlin/uninstall.sh" \
     "$ROOT/merlin/config.yaml.template" "$ROOT/merlin/version" "$pkg/"
  cp "$ROOT/docs/merlin-models.md" "$pkg/"
  local marker="$platform"
  # Legacy Koolshare checks this firmware tag before running install.sh.
  if [ "$platform" = arm ]; then marker=arm386; fi
  printf '%s\n' "$marker" > "$pkg/.valid"

  cp "$ROOT/merlin/scripts/update.sh" "$ROOT/merlin/scripts/sockrocket.sh" "$ROOT/merlin/scripts/iptables.sh" \
     "$ROOT/merlin/scripts/dnsmasq-postconf.sh" \
     "$ROOT/merlin/scripts/dnsmasq.conf" "$ROOT/merlin/scripts/sockrocket_api.sh" \
     "$ROOT/merlin/scripts/sockrocket_status.sh" \
     "$pkg/scripts/"
  cp "$ROOT/merlin/webui/sockrocket.asp" "$pkg/webui/sockrocket.asp"
  # Software Center checks webs/ before invoking install.sh. Keep a regular
  # copy: the online updater rejects archive links, including hard links.
  cp "$ROOT/merlin/webui/sockrocket.asp" "$pkg/webs/Module_sockrocket.asp"
  if [ -f "$ROOT/merlin/res/icon-sockrocket.png" ]; then
    cp "$ROOT/merlin/res/icon-sockrocket.png" "$pkg/res/icon-sockrocket.png"
  fi

  chmod +x "$pkg/install.sh" "$pkg/uninstall.sh" \
           "$pkg/scripts/sockrocket.sh" "$pkg/scripts/iptables.sh" \
           "$pkg/scripts/dnsmasq-postconf.sh" \
           "$pkg/scripts/sockrocket_api.sh" "$pkg/scripts/sockrocket_status.sh"

  local tarball="sockrocket-merlin-${platform}.tar.gz"
  # Local macOS packages must not include AppleDouble metadata or the build
  # user's account name in archive headers. GNU/BSD tar use different flags.
  if tar --version 2>/dev/null | grep -q bsdtar; then
    COPYFILE_DISABLE=1 tar --uid 0 --gid 0 --uname root --gname root \
      -C "$work" -czf "$OUT_DIR/$tarball" sockrocket
  else
    tar --owner=0 --group=0 --numeric-owner -C "$work" -czf "$OUT_DIR/$tarball" sockrocket
  fi
  rm -rf "$work"
  echo "built $OUT_DIR/$tarball"
}

# aarch64 platforms
pack_one hnd_v8 "$AARCH64_BIN"
pack_one mtk    "$AARCH64_BIN"
pack_one ipq64  "$AARCH64_BIN"

# armv7 hardfloat platforms
pack_one hnd   "$ARMV7HF_BIN"
pack_one qca   "$ARMV7HF_BIN"
pack_one ipq32 "$ARMV7HF_BIN"

# Legacy BCM470x firmware uses the soft-float ABI.
pack_one arm "$ARMV7SF_BIN"

# Standalone binaries for --online / manual replace
cp "$AARCH64_BIN" "$OUT_DIR/sockrocket-cli-merlin-aarch64"
cp "$ARMV7HF_BIN" "$OUT_DIR/sockrocket-cli-linux-armv7"
cp "$ARMV7SF_BIN" "$OUT_DIR/sockrocket-cli-linux-armv7sf"
cp "$ROOT/docs/merlin-models.md" "$OUT_DIR/"

(
  cd "$OUT_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum sockrocket-merlin-*.tar.gz sockrocket-cli-merlin-aarch64 sockrocket-cli-linux-armv7 sockrocket-cli-linux-armv7sf merlin-models.md > SHA256SUMS-merlin.txt
  else
    shasum -a 256 sockrocket-merlin-*.tar.gz sockrocket-cli-merlin-aarch64 sockrocket-cli-linux-armv7 sockrocket-cli-linux-armv7sf merlin-models.md > SHA256SUMS-merlin.txt
  fi
  cat SHA256SUMS-merlin.txt
)

echo "Merlin packages ready in $OUT_DIR"
