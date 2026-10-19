#!/usr/bin/env bash
# Pack the Linux GUI into a portable/installable tar.gz with icons + .desktop.
#
# Usage:
#   tools/linux_package.sh <gui-binary> <version> <arch-label> <out-tar.gz>
#
# Example:
#   tools/linux_package.sh target/release/sockrocket 0.1.0 x86_64 \
#     dist/sockrocket-linux-x86_64.tar.gz
set -euo pipefail

BIN="${1:?gui binary path required}"
VERSION="${2:?version required}"
ARCH="${3:?arch label required (x86_64|aarch64)}"
OUT="${4:?output .tar.gz path required}"

if [[ ! -f "$BIN" ]]; then
  echo "error: binary not found: $BIN" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ASSETS="$ROOT/crates/sockrocket-gui/assets"
if [[ ! -d "$ASSETS" ]]; then
  echo "error: missing assets: $ASSETS" >&2
  exit 1
fi

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/sockrocket-linux.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

DIR_NAME="Sockrocket-${VERSION}-linux-${ARCH}"
PKG="$STAGE/$DIR_NAME"
mkdir -p "$PKG/icons/hicolor"

cp "$BIN" "$PKG/sockrocket"
chmod +x "$PKG/sockrocket"

# Freedesktop icon theme layout
for size in 16 32 48 64 128 256 512; do
  src="$ASSETS/icon-${size}.png"
  if [[ -f "$src" ]]; then
    mkdir -p "$PKG/icons/hicolor/${size}x${size}/apps"
    cp "$src" "$PKG/icons/hicolor/${size}x${size}/apps/sockrocket.png"
  fi
done

# Desktop entry — Icon=sockrocket resolves after install.sh copies into hicolor.
cat > "$PKG/Sockrocket.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=Sockrocket
GenericName=Proxy Client
Comment=Cross-platform proxy client (Shadowsocks, VMess, VLess, Trojan, TUIC, Hysteria2)
Exec=sockrocket
Icon=sockrocket
Terminal=false
Categories=Network;Utility;
StartupNotify=true
Keywords=proxy;vpn;socks;shadowsocks;
EOF

cat > "$PKG/install.sh" <<'EOF'
#!/usr/bin/env bash
# Install Sockrocket into ~/.local (user install, no root required).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"

mkdir -p "$PREFIX/bin" "$PREFIX/share/applications" "$PREFIX/share/icons"

install -m 755 "$HERE/sockrocket" "$PREFIX/bin/sockrocket"

if [[ -d "$HERE/icons/hicolor" ]]; then
  mkdir -p "$PREFIX/share/icons/hicolor"
  cp -a "$HERE/icons/hicolor/." "$PREFIX/share/icons/hicolor/"
fi

# Rewrite Exec to absolute path so it works even if ~/.local/bin is not on PATH yet.
sed -e "s|^Exec=.*|Exec=$PREFIX/bin/sockrocket|" \
    "$HERE/Sockrocket.desktop" > "$PREFIX/share/applications/Sockrocket.desktop"
chmod 644 "$PREFIX/share/applications/Sockrocket.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$PREFIX/share/applications" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Installed Sockrocket to $PREFIX"
echo "  binary:  $PREFIX/bin/sockrocket"
echo "  desktop: $PREFIX/share/applications/Sockrocket.desktop"
echo "Launch from the app menu, or run: sockrocket"
EOF
chmod +x "$PKG/install.sh"

cat > "$PKG/uninstall.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
PREFIX="${PREFIX:-$HOME/.local}"
rm -f "$PREFIX/bin/sockrocket"
rm -f "$PREFIX/share/applications/Sockrocket.desktop"
find "$PREFIX/share/icons/hicolor" -name 'sockrocket.png' -delete 2>/dev/null || true
echo "Removed Sockrocket from $PREFIX"
EOF
chmod +x "$PKG/uninstall.sh"

cat > "$PKG/README.txt" <<EOF
Sockrocket ${VERSION} (Linux ${ARCH})
====================================

Quick start (portable):
  ./sockrocket

Install (adds app menu icon):
  ./install.sh

Uninstall:
  ./uninstall.sh

Default listeners:
  SOCKS5  127.0.0.1:1080
  HTTP    127.0.0.1:1087

Docs: https://github.com/sockrockets/sockrocket
EOF

mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
tar -C "$STAGE" -czf "$OUT" "$DIR_NAME"
echo "Built: $OUT"
