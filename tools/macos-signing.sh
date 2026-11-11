#!/usr/bin/env bash
#
# Set a Mac up to sign Mote with its own certificate, for
# tools/build-macos-app.sh.
#
#   MOTE_SIGN_P12_PASSWORD=... tools/macos-signing.sh mote-signing.p12
#
# Mote isn't signed by Apple (there is no Developer ID), so it is signed
# with a self-signed certificate of its own. macOS knows an app by its
# signature: with the same certificate on every build, the Keychain keeps
# handing a new version the passwords the last one saved, the microphone
# and local network stay allowed, and Sparkle accepts the update as coming
# from the same app. The certificate and its key are kept by the maintainer
# and, for the release workflow, in the repository's secrets; never in the
# repository.
#
# The certificate goes into a keychain of its own, with a password of its
# own, which unlocks over SSH and in CI, where the login keychain doesn't.
# The keychain's details go in ~/.config/mote-signing/env (or
# $MOTE_SIGNING_ENV), which the build reads.
set -euo pipefail

P12="${1:?usage: MOTE_SIGN_P12_PASSWORD=... $0 <certificate.p12>}"
: "${MOTE_SIGN_P12_PASSWORD:?the password of the .p12 file}"
KEYCHAIN="${MOTE_SIGN_KEYCHAIN:-$HOME/Library/Keychains/mote-signing.keychain-db}"
ENV_FILE="${MOTE_SIGNING_ENV:-$HOME/.config/mote-signing/env}"
PASSWORD="$(openssl rand -hex 24)"

security delete-keychain "$KEYCHAIN" 2>/dev/null || true
security create-keychain -p "$PASSWORD" "$KEYCHAIN"
# No locking after a while or on sleep: the build unlocks it each time.
security set-keychain-settings "$KEYCHAIN"
security unlock-keychain -p "$PASSWORD" "$KEYCHAIN"
security import "$P12" -k "$KEYCHAIN" -P "$MOTE_SIGN_P12_PASSWORD" -T /usr/bin/codesign >/dev/null
# codesign may use the key without asking.
security set-key-partition-list -S apple-tool:,apple: -s -k "$PASSWORD" "$KEYCHAIN" >/dev/null
IDENTITY="$(security find-identity -p codesigning "$KEYCHAIN" | awk '/\)/ { print $2; exit }')"
[ -n "$IDENTITY" ] || { echo "no signing identity in $P12" >&2; exit 1; }

mkdir -p "$(dirname "$ENV_FILE")"
umask 077
cat > "$ENV_FILE" <<EOF
MOTE_SIGN_IDENTITY=$IDENTITY
MOTE_SIGN_KEYCHAIN='$KEYCHAIN'
MOTE_SIGN_KEYCHAIN_PASSWORD=$PASSWORD
EOF
echo "signing as $IDENTITY, from $KEYCHAIN"
