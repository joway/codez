#!/usr/bin/env bash
#
# Package CodeZ into a macOS .app bundle and a distributable .dmg.
#
#   ./scripts/package-macos.sh
#
# Output lands in dist/:
#   dist/CodeZ.app          — the application bundle (drag to /Applications)
#   dist/CodeZ-<ver>.dmg    — a disk image with an /Applications shortcut
#
set -euo pipefail

# --- locate project root (this script lives in scripts/) ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

APP_NAME="CodeZ"
BIN_NAME="codez"                       # cargo package / binary name
BUNDLE_ID="com.codez.app"
ICON_SRC="assets/logo-1024.png"        # source logo; .icns is generated from it
ICON_FALLBACK="assets/CodeZ.icns"      # used if the source logo is missing
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
CONTENTS="$APP/Contents"

echo "==> Building $APP_NAME $VERSION (universal: arm64 + x86_64)"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
for t in "${TARGETS[@]}"; do
  rustup target add "$t" >/dev/null 2>&1 || true
  echo "    cargo build --release --target $t"
  cargo build --release --target "$t"
done

echo "==> Assembling $APP_NAME.app"
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

# Combine the per-arch binaries into one universal2 executable.
lipo -create -output "$CONTENTS/MacOS/$APP_NAME" \
  "target/aarch64-apple-darwin/release/$BIN_NAME" \
  "target/x86_64-apple-darwin/release/$BIN_NAME"
chmod +x "$CONTENTS/MacOS/$APP_NAME"

echo "==> Generating app icon"
if [[ -f "$ICON_SRC" ]]; then
  # Build a fresh .icns from the logo so the bundle always matches assets/.
  ICONSET="$(mktemp -d)/$APP_NAME.iconset"
  mkdir -p "$ICONSET"
  for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$ICON_SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    sips -z "$((s * 2))" "$((s * 2))" "$ICON_SRC" \
      --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$CONTENTS/Resources/$APP_NAME.icns"
  rm -rf "$(dirname "$ICONSET")"
  # Keep the committed .icns in sync too.
  cp "$CONTENTS/Resources/$APP_NAME.icns" "$ICON_FALLBACK"
elif [[ -f "$ICON_FALLBACK" ]]; then
  cp "$ICON_FALLBACK" "$CONTENTS/Resources/$APP_NAME.icns"
else
  echo "    (no logo at $ICON_SRC — bundling without an icon)"
fi

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleExecutable</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>$APP_NAME</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

# Ad-hoc sign so the app launches locally without "damaged/unverified" errors.
echo "==> Ad-hoc code signing"
codesign --force --deep --sign - "$APP" 2>/dev/null || \
  echo "    (codesign unavailable — skipping; app still runs from Finder)"

echo "==> Creating .dmg"
DMG="$DIST/$APP_NAME-$VERSION.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

echo
echo "Done:"
echo "  $APP"
echo "  $DMG"
echo
echo "Install: open the .dmg and drag $APP_NAME to Applications."
echo "CLI:     ./scripts/install-cli.sh   (adds a 'codez' command to open it)"
echo
echo "If the Dock/Finder shows a stale icon after reinstalling, refresh with:"
echo "  touch /Applications/$APP_NAME.app && killall Dock Finder"
