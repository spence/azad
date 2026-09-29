#!/usr/bin/env bash
# Regenerates reference/vhid_reference.txt from the pinned Karabiner-DriverKit-VirtualHIDDevice
# headers. The Rust client's unit test compares its encoding against that file.
set -euo pipefail

REVISION="ba98de7fae2d529b9debe82890765dc66246f4ff" # v8.6.0: driver 1.8.0, client protocol 7
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT_DIR="$(cd "$CRATE_DIR/../.." && pwd)"
WORK="${ROOT_DIR}/target/vhid-reference"
SOURCE="${WORK}/driver-${REVISION}"

if [[ ! -d "$SOURCE/include" ]]; then
  rm -rf "$SOURCE"
  mkdir -p "$WORK"
  git clone --quiet --filter=blob:none --no-checkout \
    https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice.git "$SOURCE"
  git -C "$SOURCE" checkout --quiet "$REVISION"
fi

clang++ -std=c++23 -O0 \
  -I"$SOURCE/include" -I"$SOURCE/vendor/vendor/include" \
  "$CRATE_DIR/reference/vhid_reference.cpp" -o "$WORK/vhid_reference"
"$WORK/vhid_reference" > "$CRATE_DIR/reference/vhid_reference.txt"
echo "wrote $CRATE_DIR/reference/vhid_reference.txt"
