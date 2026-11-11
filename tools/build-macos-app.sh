#!/usr/bin/env bash
#
# Build the macOS app, on a Mac with Homebrew's GTK and libadwaita.
#
#   tools/build-macos-app.sh                # target/debug/Mote.app
#   tools/build-macos-app.sh release        # target/release/Mote.app
#   tools/build-macos-app.sh release dmg    # and target/release/Mote-<version>.dmg
#
# The bundle carries everything it runs on: GTK, libadwaita and the
# libraries under them, the data GTK reads (tools/macos-bundle.py), and
# Sparkle, which updates it from the appcast on Mote's site. A version
# with a pre-release part (1.5.0-beta.1) builds the beta app, "Mote Beta",
# which checks the beta appcast.
#
# It is signed with Mote's own certificate when one is set up (see
# tools/macos-signing.sh): macOS then knows each new build as the same app,
# so the Keychain keeps handing it saved passwords, permissions stay given,
# and Sparkle takes the update. Without one it is signed ad hoc, which suits
# a build that never leaves the Mac it was made on.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-debug}"
DMG="${2:-}"
cd "$ROOT"

# Sparkle, from its release on GitHub, checked against the archive's SHA-256.
SPARKLE_VERSION=2.10.1
SPARKLE_SHA256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
# Where updates come from, and the public half of the key Mote's site signs
# them with (Flatout keeps the private half).
FEED=https://mote.hyprlab.co/sparkle/appcast.xml
BETA_FEED=https://mote.hyprlab.co/sparkle/appcast-beta.xml
ED_PUBLIC_KEY=mN3ZnyLRNBstXPHvgB+Cs4qLS6YZtOZRLviznrhtSx4=

# The signing setup tools/macos-signing.sh writes, if there is one.
SIGNING_ENV="${MOTE_SIGNING_ENV:-$HOME/.config/mote-signing/env}"
# shellcheck source=/dev/null
[ -f "$SIGNING_ENV" ] && . "$SIGNING_ENV"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
FEATURES=()
if [[ "$VERSION" == *-* ]]; then
  NAME="Mote (beta)" ID=co.hyprlab.Mote.Beta BUNDLE="Mote Beta" FEED="$BETA_FEED"
  FEATURES=(--features beta)
else
  NAME=Mote ID=co.hyprlab.Mote BUNDLE=Mote
fi

if [ "$PROFILE" = release ]; then
  cargo build --release ${FEATURES[@]+"${FEATURES[@]}"}
else
  cargo build ${FEATURES[@]+"${FEATURES[@]}"}
fi

APP="target/$PROFILE/$BUNDLE.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "target/$PROFILE/mote" "$APP/Contents/MacOS/Mote"

# The icon, at every size iconutil wants, from the 1024 px macOS one.
ICONSET="$(mktemp -d)/Mote.iconset"
mkdir -p "$ICONSET"
SOURCE=data/icons/macos/co.hyprlab.Mote.png
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SOURCE" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$SOURCE" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Mote.icns"
rm -rf "$(dirname "$ICONSET")"

# LSMinimumSystemVersion is filled in by macos-bundle.py.
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleExecutable</key><string>Mote</string>
  <key>CFBundleIconFile</key><string>Mote</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>The microphone goes to the remote desktop while a session asks for it.</string>
  <key>NSLocalNetworkUsageDescription</key><string>Finds remote desktops on the local network.</string>
  <key>SUFeedURL</key><string>$FEED</string>
  <key>SUPublicEDKey</key><string>$ED_PUBLIC_KEY</string>
</dict>
</plist>
EOF

python3 tools/macos-bundle.py "$APP"

# Sparkle. Its XPC services are for sandboxed apps, which Mote isn't.
SPARKLE_DIR="target/sparkle-$SPARKLE_VERSION"
if [ ! -d "$SPARKLE_DIR/Sparkle.framework" ]; then
  mkdir -p "$SPARKLE_DIR"
  curl -fsSLo "$SPARKLE_DIR/sparkle.tar.xz" \
    "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
  echo "$SPARKLE_SHA256  $SPARKLE_DIR/sparkle.tar.xz" | shasum -a 256 -c - >/dev/null
  tar -xJf "$SPARKLE_DIR/sparkle.tar.xz" -C "$SPARKLE_DIR"
fi
ditto "$SPARKLE_DIR/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
rm -rf "$APP/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices" \
  "$APP/Contents/Frameworks/Sparkle.framework/XPCServices"

# Sign from the inside out: every library, Sparkle's helpers, then the app.
IDENTITY="${MOTE_SIGN_IDENTITY:--}"
SIGN=(codesign --force --timestamp=none --sign "$IDENTITY")
if [ -n "${MOTE_SIGN_KEYCHAIN:-}" ]; then
  security unlock-keychain -p "${MOTE_SIGN_KEYCHAIN_PASSWORD:-}" "$MOTE_SIGN_KEYCHAIN"
  SIGN+=(--keychain "$MOTE_SIGN_KEYCHAIN")
fi
[ "$IDENTITY" = - ] && echo "note: signed ad hoc; set up tools/macos-signing.sh for a build to give out" >&2
# codesign says "replacing existing signature" for every library; only a
# failure is worth showing.
sign() {
  local out
  if ! out="$("${SIGN[@]}" "$@" 2>&1)"; then
    echo "$out" >&2
    exit 1
  fi
}
SPARKLE_B="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
while IFS= read -r -d '' library; do
  sign "$library"
done < <(find "$APP/Contents/Frameworks" "$APP/Contents/PlugIns" -maxdepth 2 -type f \( -name '*.dylib' -o -name '*.so' \) -print0)
sign "$SPARKLE_B/Autoupdate"
sign "$SPARKLE_B/Updater.app"
sign "$APP/Contents/Frameworks/Sparkle.framework"
sign "$APP"
codesign --verify --deep --strict "$APP"
echo "$APP"

if [ "$DMG" = dmg ]; then
  OUT="target/$PROFILE/${BUNDLE// /-}-$VERSION.dmg"
  STAGE="$(mktemp -d)"
  ditto "$APP" "$STAGE/$BUNDLE.app"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$OUT"
  hdiutil create -quiet -volname "$NAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO "$OUT"
  rm -rf "$STAGE"
  sign --identifier "$ID.dmg" "$OUT"
  echo "$OUT"
fi
