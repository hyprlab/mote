#!/usr/bin/env bash
#
# Wrap a build in a macOS app bundle, on a Mac.
#
#   tools/build-macos-app.sh            # target/debug/Mote.app
#   tools/build-macos-app.sh release    # target/release/Mote.app
#
# The bundle is what gives the app its name in the menu bar and the Dock,
# its icon, and the descriptions macOS shows when it asks for the
# microphone or the local network; a bare binary goes by its file name.
#
# It is for development: the binary still loads GTK and libadwaita from
# Homebrew, and it is signed ad hoc, so each build is a new app to the
# Keychain, which asks again before handing over a saved password.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-debug}"
cd "$ROOT"

if [ "$PROFILE" = release ]; then
  cargo build --release
else
  cargo build
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="target/$PROFILE/Mote.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/mote" "$APP/Contents/MacOS/Mote"

# The icon, at every size iconutil wants, from the 512 px PNG.
ICONSET="$(mktemp -d)/Mote.iconset"
mkdir -p "$ICONSET"
SOURCE=data/icons/hicolor/512x512/apps/co.hyprlab.Mote.png
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SOURCE" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  [ "$double" -le 512 ] &&
    sips -z "$double" "$double" "$SOURCE" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
cp "$SOURCE" "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Mote.icns"
rm -rf "$(dirname "$ICONSET")"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>co.hyprlab.Mote</string>
  <key>CFBundleName</key><string>Mote</string>
  <key>CFBundleDisplayName</key><string>Mote</string>
  <key>CFBundleExecutable</key><string>Mote</string>
  <key>CFBundleIconFile</key><string>Mote</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>The microphone goes to the remote desktop while a session asks for it.</string>
  <key>NSLocalNetworkUsageDescription</key><string>Finds remote desktops on the local network.</string>
</dict>
</plist>
EOF

codesign --force --sign - "$APP" 2>/dev/null
echo "$APP"
