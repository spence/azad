#!/usr/bin/env bash
# Installs the Karabiner DriverKit VirtualHIDDevice package Azad's keyboard capture forwards
# typing through, then asks macOS to activate its driver extension. Needs an administrator
# password, and the user must allow the extension in System Settings when prompted.
# A Mac that already runs Karabiner-Elements has this driver and needs nothing here.
set -euo pipefail

VERSION="8.6.0"
SHA256="ff8c7fdc5e25387c7805fc7509a0fa9cf98f69ba582704f717fddcae47424387"
URL="https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/releases/download/v${VERSION}/Karabiner-DriverKit-VirtualHIDDevice-${VERSION}.pkg"
MANAGER="/Applications/.Karabiner-VirtualHIDDevice-Manager.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Manager"

if systemextensionsctl list 2>/dev/null | grep -q 'org.pqrs.Karabiner-DriverKit-VirtualHIDDevice.*activated enabled'; then
  echo "Virtual keyboard driver already active:"
  systemextensionsctl list | grep 'org.pqrs.Karabiner-DriverKit-VirtualHIDDevice'
  exit 0
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/azad-capture-driver.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
curl -fsSL -o "$WORK/driver.pkg" "$URL"
echo "${SHA256}  $WORK/driver.pkg" | shasum -a 256 -c -
pkgutil --check-signature "$WORK/driver.pkg" | grep -q 'Developer ID Installer: Fumihiko Takayama (G43BCU2T37)' || {
  echo "error: unexpected package signature" >&2
  pkgutil --check-signature "$WORK/driver.pkg" >&2
  exit 1
}
sudo installer -pkg "$WORK/driver.pkg" -target /
"$MANAGER" activate
echo "Allow the Karabiner DriverKit VirtualHIDDevice extension in System Settings ->"
echo "General -> Login Items & Extensions -> Driver Extensions, then run: just status"
