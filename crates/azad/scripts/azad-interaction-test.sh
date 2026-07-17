#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

cargo build -q -p azad --bin azad-interaction-harness
BIN="$ROOT/target/debug/azad-interaction-harness"

description="$($BIN describe)"
for required in \
  '"process_local_events":true' \
  '"registers_global_hotkeys":false' \
  '"posts_core_graphics_events":false' \
  '"opens_appkit_windows":false' \
  '"reads_or_writes_user_defaults":false' \
  '"opens_microphone":false' \
  '"performs_paste":false'
do
  if ! rg -q -F "$required" <<<"$description"; then
    echo "interaction harness safety declaration is missing: $required" >&2
    exit 1
  fi
done

if otool -L "$BIN" | rg -q 'AppKit|CoreGraphics|Carbon|AVFoundation|CoreAudio|IOKit'; then
  echo "interaction harness links a forbidden desktop framework" >&2
  otool -L "$BIN" >&2
  exit 1
fi

if nm -u "$BIN" | rg -q 'CGEvent|RegisterEventHotKey|NSApp|NSUserDefaults|AVCapture|AudioUnit|AXUIElement|IOHID'; then
  echo "interaction harness imports a forbidden desktop API" >&2
  nm -u "$BIN" | rg 'CGEvent|RegisterEventHotKey|NSApp|NSUserDefaults|AVCapture|AudioUnit|AXUIElement|IOHID' >&2
  exit 1
fi

"$BIN" self-test
