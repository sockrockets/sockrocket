#!/usr/bin/env bash
# Build a Sockrocket.app bundle and pack it into a UDZO .dmg (with Applications link).
#
# Usage:
#   tools/macos_dmg.sh <gui-binary> <version> <arch-label> <out-dmg> [app-dir]
#
# Example:
#   tools/macos_dmg.sh target/release/sockrocket 0.1.0 aarch64 \
#     dist/sockrocket-macos-aarch64.dmg
#
# Optional 5th arg: reuse/write the .app at that path (default: alongside the dmg).
set -euo pipefail

BIN="${1:?gui binary path required}"
VERSION="${2:?version required}"
ARCH="${3:?arch label required (e.g. aarch64, x86_64)}"
OUT_DMG="${4:?output .dmg path required}"
APP_DIR="${5:-}"

if [[ ! -f "$BIN" ]]; then
  echo "error: binary not found: $BIN" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ICON="$ROOT/crates/sockrocket-gui/assets/icon.icns"
if [[ ! -f "$ICON" ]]; then
  echo "error: missing icon: $ICON" >&2
  exit 1
fi

if [[ -z "$APP_DIR" ]]; then
  APP_DIR="$(dirname "$OUT_DMG")/Sockrocket.app"
fi

BUNDLE_ID="io.github.sockrockets.sockrocket"
APP_NAME="Sockrocket"

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$BIN" "$APP_DIR/Contents/MacOS/sockrocket"
chmod +x "$APP_DIR/Contents/MacOS/sockrocket"
cp "$ICON" "$APP_DIR/Contents/Resources/AppIcon.icns"

# Minimal Info.plist — enough for Finder / Gatekeeper / Launch Services.
cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>sockrocket</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleSupportedPlatforms</key>
  <array>
    <string>MacOSX</string>
  </array>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright © Sockrocket. Apache-2.0.</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
</dict>
</plist>
EOF

# Ad-hoc sign so the bundle is at least structurally valid when no Developer ID
# secrets are present. Real release signing overwrites this in CI.
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true
fi

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/sockrocket-dmg.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

cp -R "$APP_DIR" "$STAGE/${APP_NAME}.app"
ln -s /Applications "$STAGE/Applications"

# Volume name stays short for Finder; arch is in the filename.
VOL_NAME="Sockrocket ${VERSION}"
mkdir -p "$(dirname "$OUT_DMG")"
rm -f "$OUT_DMG"

hdiutil create \
  -volname "$VOL_NAME" \
  -srcfolder "$STAGE" \
  -ov \
  -format UDZO \
  -fs HFS+ \
  "$OUT_DMG"

echo "Built app:  $APP_DIR"
echo "Built dmg:  $OUT_DMG  (arch=${ARCH})"
