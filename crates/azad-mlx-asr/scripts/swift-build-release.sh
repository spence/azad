#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: swift-build-release.sh <package-path> <scratch-path>" >&2
  exit 2
fi

PACKAGE_PATH="$1"
SCRATCH_PATH="$2"
STDERR_LOG="$(mktemp)"

cleanup() {
  rm -f "$STDERR_LOG"
}
trap cleanup EXIT

if swift build -c release --package-path "$PACKAGE_PATH" --scratch-path "$SCRATCH_PATH" 2>"$STDERR_LOG"; then
  STATUS=0
else
  STATUS=$?
fi

awk '
  pending != "" {
    if ($0 ~ /Sources\/MLXAudioVAD\/Models\/SileroVAD\/README\.md$/) {
      pending = ""
      next
    }
    print pending > "/dev/stderr"
    pending = ""
  }
  /^warning: '\''mlx-audio-swift'\'': found 1 file\(s\) which are unhandled; explicitly declare them as resources or exclude from the target$/ {
    pending = $0
    next
  }
  { print > "/dev/stderr" }
  END {
    if (pending != "") {
      print pending > "/dev/stderr"
    }
  }
' "$STDERR_LOG"

exit "$STATUS"
