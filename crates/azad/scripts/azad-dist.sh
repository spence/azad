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

RELEASE_CONFIG_FILE="${AZAD_RELEASE_CONFIG:-$ROOT_DIR/.release.env}"
if [[ -f "$RELEASE_CONFIG_FILE" ]]; then
  # Local, ignored shell assignments for official release packaging.
  source "$RELEASE_CONFIG_FILE"
fi

for var in \
  AZAD_VERSION \
  AZAD_SIGNING_IDENTITY \
  AZAD_NOTARIZATION_PROFILE; do
  restore_env_var "$var"
done

LABEL="ai.azad"
VERSION="${AZAD_VERSION:-0.2.0}"
SIGNING_IDENTITY="${AZAD_SIGNING_IDENTITY:-}"
NOTARIZATION_PROFILE="${AZAD_NOTARIZATION_PROFILE:-azad-notarization}"

DIST_DIR="${ROOT_DIR}/dist"
APP_DIR="${DIST_DIR}/Azad.app"
APP_CONTENTS="${APP_DIR}/Contents"
APP_MACOS="${APP_CONTENTS}/MacOS"
APP_RESOURCES="${APP_CONTENTS}/Resources"
MLX_HELPER_DIR="${ROOT_DIR}/crates/azad-mlx-asr"
MLX_HELPER_BUILD_DIR="${ROOT_DIR}/target/swift/azad-mlx-asr"
MLX_HELPER_SOURCE=""
MLX_METALLIB_SOURCE=""
UI_DIR="${ROOT_DIR}/crates/azad-ui"
UI_BUILD_DIR="${ROOT_DIR}/target/swift/azad-ui"
UI_LIB_SOURCE=""
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
  AZAD_VERSION             Version string (default: 0.2.0)
  AZAD_NOTARIZATION_PROFILE  notarytool credential profile (default: azad-notarization)
                             Create with: xcrun notarytool store-credentials "azad-notarization"
  AZAD_RELEASE_CONFIG      Local env file (default: <workspace>/.release.env)

USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "${1:-}" == "help" ]]; then
  usage
  exit 0
fi

build_mlx_helper() {
  if [[ ! -f "${MLX_HELPER_DIR}/Package.swift" ]]; then
    echo "error: MLX ASR helper package missing at ${MLX_HELPER_DIR}" >&2
    exit 1
  fi

  if ! command -v swift >/dev/null 2>&1; then
    echo "error: swift not found; install Xcode Command Line Tools to build the MLX ASR helper" >&2
    exit 1
  fi

  "${ROOT_DIR}/crates/azad-mlx-asr/scripts/swift-build-release.sh" \
    "$MLX_HELPER_DIR" \
    "$MLX_HELPER_BUILD_DIR"

  MLX_HELPER_SOURCE="${MLX_HELPER_BUILD_DIR}/release/azad-mlx-asr"
  if [[ ! -x "$MLX_HELPER_SOURCE" ]]; then
    echo "error: built MLX ASR helper missing at $MLX_HELPER_SOURCE" >&2
    exit 1
  fi

  build_mlx_metallib
}

build_ui_library() {
  if [[ ! -f "${UI_DIR}/Package.swift" ]]; then
    echo "error: Azad UI package missing at ${UI_DIR}" >&2
    exit 1
  fi

  if ! command -v swift >/dev/null 2>&1; then
    echo "error: swift not found; install Xcode Command Line Tools to build the Azad UI library" >&2
    exit 1
  fi

  "${ROOT_DIR}/crates/azad-mlx-asr/scripts/swift-build-release.sh" \
    "$UI_DIR" \
    "$UI_BUILD_DIR"

  UI_LIB_SOURCE="${UI_BUILD_DIR}/release/libAzadUI.dylib"
  if [[ ! -f "$UI_LIB_SOURCE" ]]; then
    echo "error: built Azad UI library missing at $UI_LIB_SOURCE" >&2
    exit 1
  fi
}

ensure_metal_toolchain() {
  if xcrun --find metal >/dev/null 2>&1; then
    return
  fi

  if [[ -d /Applications/Xcode.app/Contents/Developer ]]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
  fi

  if xcrun --find metal >/dev/null 2>&1; then
    return
  fi

  if command -v xcodebuild >/dev/null 2>&1; then
    echo "Installing Apple Metal Toolchain component for MLX..."
    xcodebuild -downloadComponent MetalToolchain || true
  fi

  if ! xcrun --find metal >/dev/null 2>&1; then
    echo "error: Apple Metal compiler not available; install Xcode's Metal Toolchain component" >&2
    echo "hint: DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcodebuild -downloadComponent MetalToolchain" >&2
    exit 1
  fi
}

build_mlx_metallib() {
  ensure_metal_toolchain

  local metal_src="${MLX_HELPER_BUILD_DIR}/checkouts/mlx-swift/Source/Cmlx/mlx-generated/metal"
  local metal_build_dir="${MLX_HELPER_BUILD_DIR}/release/mlx-metallib-build"
  local out_lib="${MLX_HELPER_BUILD_DIR}/release/mlx.metallib"

  if [[ ! -d "$metal_src" ]]; then
    echo "error: MLX generated Metal sources missing at $metal_src" >&2
    exit 1
  fi

  rm -rf "$metal_build_dir"
  mkdir -p "$metal_build_dir"

  local airs=()
  local src rel stem air
  while IFS= read -r src; do
    rel="${src#${metal_src}/}"
    stem="${rel%.metal}"
    stem="${stem//\//_}"
    air="${metal_build_dir}/${stem}.air"
    (
      cd "$metal_src"
      xcrun -sdk macosx metal \
        -x metal \
        -Wall \
        -Wextra \
        -fno-fast-math \
        -Wno-c++17-extensions \
        -Wno-c++20-extensions \
        -mmacosx-version-min=14.0 \
        -c "$rel" \
        -I. \
        -o "$air"
    )
    airs+=("$air")
  done < <(find "$metal_src" -name '*.metal' -type f | sort)

  if [[ "${#airs[@]}" == "0" ]]; then
    echo "error: no MLX Metal kernels found under $metal_src" >&2
    exit 1
  fi

  xcrun -sdk macosx metallib "${airs[@]}" -o "$out_lib"
  MLX_METALLIB_SOURCE="$out_lib"
}

if [[ -z "$SIGNING_IDENTITY" ]]; then
  echo "error: set AZAD_SIGNING_IDENTITY to your Developer ID Application identity" >&2
  echo "error: local release settings can also go in: $RELEASE_CONFIG_FILE" >&2
  exit 1
fi

echo "==> Building release binary"
pushd "$CRATE_DIR" >/dev/null
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release
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

echo "==> Building Azad UI library"
build_ui_library

echo "==> Building MLX ASR helper"
build_mlx_helper

echo "==> Assembling app bundle"
rm -rf "$APP_DIR"
mkdir -p "$APP_MACOS" "$APP_RESOURCES"

install -m 755 "$BIN_SOURCE" "${APP_MACOS}/azad"
install -m 755 "$UI_LIB_SOURCE" "${APP_MACOS}/libAzadUI.dylib"
install -m 755 "$MLX_HELPER_SOURCE" "${APP_MACOS}/azad-mlx-asr"
install -m 644 "$MLX_METALLIB_SOURCE" "${APP_MACOS}/mlx.metallib"
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
  <string>14.0</string>
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
  --no-progress \
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

echo "==> Signing DMG"
/usr/bin/codesign \
  --force \
  --sign "$SIGNING_IDENTITY" \
  --timestamp \
  "$DMG_PATH"

echo "==> Notarizing DMG"
xcrun notarytool submit "$DMG_PATH" \
  --keychain-profile "$NOTARIZATION_PROFILE" \
  --no-progress \
  --wait

echo "==> Stapling notarization ticket to DMG"
xcrun stapler staple "$DMG_PATH"

echo ""
echo "Distribution complete:"
echo "  App:  $APP_DIR"
echo "  DMG:  $DMG_PATH"
