#!/usr/bin/env bash
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT_DIR="$(cd "${CRATE_DIR}/../.." && pwd)"

capture_env_var() {
  local name="$1"
  local has_var="AZAD_ENV_HAS_${name}"
  local value_var="AZAD_ENV_VALUE_${name}"
  if [[ "${!name+x}" == "x" ]]; then
    printf -v "$has_var" '%s' "1"
    printf -v "$value_var" '%s' "${!name}"
  else
    printf -v "$has_var" '%s' "0"
    printf -v "$value_var" '%s' ""
  fi
}

restore_env_var() {
  local name="$1"
  local has_var="AZAD_ENV_HAS_${name}"
  local value_var="AZAD_ENV_VALUE_${name}"
  if [[ "${!has_var}" == "1" ]]; then
    printf -v "$name" '%s' "${!value_var}"
  fi
}

for var in \
  AZAD_VERSION \
  AZAD_SIGNING_IDENTITY \
  AZAD_NOTARIZATION_PROFILE; do
  capture_env_var "$var"
done

CONFIG_FILE="${AZAD_CONFIG:-$ROOT_DIR/.codesign.env}"
if [[ -f "$CONFIG_FILE" ]]; then
  # Local, ignored shell assignments for release signing/notarization settings.
  source "$CONFIG_FILE"
fi

for var in \
  AZAD_VERSION \
  AZAD_SIGNING_IDENTITY \
  AZAD_NOTARIZATION_PROFILE; do
  restore_env_var "$var"
done

LABEL="ai.azad"
VERSION="${AZAD_VERSION:-0.1.0}"
SIGNING_IDENTITY="${AZAD_SIGNING_IDENTITY:-}"
NOTARIZATION_PROFILE="${AZAD_NOTARIZATION_PROFILE:-azad-notarization}"

DIST_DIR="${ROOT_DIR}/dist"
APP_DIR="${DIST_DIR}/Azad.app"
APP_CONTENTS="${APP_DIR}/Contents"
APP_MACOS="${APP_CONTENTS}/MacOS"
APP_RESOURCES="${APP_CONTENTS}/Resources"
DMG_NAME="Azad-${VERSION}.dmg"
DMG_PATH="${DIST_DIR}/${DMG_NAME}"

usage() {
  cat <<'USAGE'
Usage: scripts/azad-dist.sh

Builds a release Azad.app, signs it with a Developer ID, notarizes it,
and packages it into a DMG for distribution.

Required environment variables:
  AZAD_SIGNING_IDENTITY    Developer ID Application identity (name or hash)

Optional environment variables:
  AZAD_VERSION             Version string (default: 0.1.0)
  AZAD_NOTARIZATION_PROFILE  notarytool credential profile (default: azad-notarization)
                             Create with: xcrun notarytool store-credentials "azad-notarization"
  AZAD_CONFIG              Local env file (default: <workspace>/.codesign.env)

USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "${1:-}" == "help" ]]; then
  usage
  exit 0
fi

if [[ -z "$SIGNING_IDENTITY" ]]; then
  echo "error: set AZAD_SIGNING_IDENTITY to your Developer ID Application identity" >&2
  echo "error: local release settings can also go in: $CONFIG_FILE" >&2
  exit 1
fi

echo "==> Building release binary"
pushd "$CRATE_DIR" >/dev/null
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --release
popd >/dev/null

target_dir="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
if [[ -n "${CARGO_TARGET_DIR:-}" && "$target_dir" != /* ]]; then
  target_dir="${CRATE_DIR}/${target_dir}"
fi

BIN_SOURCE="${target_dir}/release/azad"
if [[ ! -x "$BIN_SOURCE" ]]; then
  echo "error: release binary not found at $BIN_SOURCE" >&2
  exit 1
fi

echo "==> Assembling app bundle"
rm -rf "$APP_DIR"
mkdir -p "$APP_MACOS" "$APP_RESOURCES"

install -m 755 "$BIN_SOURCE" "${APP_MACOS}/azad"
install -m 644 "${CRATE_DIR}/assets/azad-black.png" "${APP_RESOURCES}/azad-black.png"
install -m 644 "${CRATE_DIR}/assets/azad-white.png" "${APP_RESOURCES}/azad-white.png"
install -m 644 "${CRATE_DIR}/assets/azad.icns" "${APP_RESOURCES}/azad.icns"
install -m 644 "${CRATE_DIR}/assets/claude.svg" "${APP_RESOURCES}/claude.svg"

cat >"${APP_CONTENTS}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>azad</string>
  <key>CFBundleIdentifier</key>
  <string>${LABEL}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleIconFile</key>
  <string>azad.icns</string>
  <key>CFBundleName</key>
  <string>Azad</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Azad uses microphone audio to transcribe your speech.</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
</dict>
</plist>
EOF

echo "==> Signing app bundle"
/usr/bin/codesign \
  --force \
  --deep \
  --options runtime \
  --sign "$SIGNING_IDENTITY" \
  --entitlements "$CRATE_DIR/Azad.entitlements" \
  --timestamp \
  "$APP_DIR"

echo "==> Notarizing app bundle"
APP_ZIP="${DIST_DIR}/Azad.zip"
ditto -c -k --keepParent "$APP_DIR" "$APP_ZIP"
xcrun notarytool submit "$APP_ZIP" \
  --keychain-profile "$NOTARIZATION_PROFILE" \
  --wait
rm -f "$APP_ZIP"

echo "==> Stapling notarization ticket to app"
xcrun stapler staple "$APP_DIR"

echo "==> Creating DMG"
STAGING_DIR="$(mktemp -d)"
cp -a "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

rm -f "$DMG_PATH"
hdiutil create \
  -volname "Azad" \
  -srcfolder "$STAGING_DIR" \
  -format UDZO \
  "$DMG_PATH"

rm -rf "$STAGING_DIR"

echo "==> Notarizing DMG"
xcrun notarytool submit "$DMG_PATH" \
  --keychain-profile "$NOTARIZATION_PROFILE" \
  --wait

echo "==> Stapling notarization ticket to DMG"
xcrun stapler staple "$DMG_PATH"

echo ""
echo "Distribution complete:"
echo "  App:  $APP_DIR"
echo "  DMG:  $DMG_PATH"
