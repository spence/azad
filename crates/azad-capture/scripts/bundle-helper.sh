#!/usr/bin/env bash
# Builds and signs `Azad Capture.app`, the root keyboard-capture helper bundle.
# Usage: bundle-helper.sh <output-dir> [signing-identity]
# The bundle identity (ai.azad.capture + team) is what Input Monitoring records, so it must stay
# stable across updates for the grant to survive.
set -euo pipefail

OUT="$1"
IDENTITY="${2:-${AZAD_CODESIGN_IDENTITY:-}}"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT_DIR="$(cd "$CRATE_DIR/../.." && pwd)"
VERSION="$(awk -F '"' '/^version =/ { print $2; exit }' "$ROOT_DIR/Cargo.toml")"

cargo build --release -p azad-capture --manifest-path "$ROOT_DIR/Cargo.toml"
BUNDLE="$OUT/Azad Capture.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS"
cp "${CARGO_TARGET_DIR:-$ROOT_DIR/target}/release/azad-capture" "$BUNDLE/Contents/MacOS/azad-capture"
sed "s/__VERSION__/$VERSION/g" "$CRATE_DIR/bundle/Info.plist" > "$BUNDLE/Contents/Info.plist"
if [[ -n "$IDENTITY" ]]; then
  codesign --force --sign "$IDENTITY" --options runtime --timestamp "$BUNDLE"
else
  codesign --force --sign - "$BUNDLE"
fi
codesign --verify --strict "$BUNDLE"
echo "$BUNDLE"
