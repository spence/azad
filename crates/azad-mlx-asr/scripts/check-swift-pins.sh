#!/usr/bin/env bash
set -euo pipefail

package_resolved="${1:-crates/azad-mlx-asr/Package.resolved}"
status=0

pin_version() {
  local identity="$1"
  awk -v wanted="\"identity\" : \"${identity}\"" '
    $0 ~ wanted { in_pin = 1 }
    in_pin && /"version" :/ {
      version = $3
      gsub(/[",]/, "", version)
      print version
      exit
    }
  ' "$package_resolved"
}

check_pin() {
  local identity="$1"
  local expected="$2"
  local actual

  actual="$(pin_version "$identity")"
  if [[ "$actual" == "$expected" ]]; then
    printf "ok  %s: %s\n" "$identity" "$actual"
  else
    printf "err %s: expected %s, found %s\n" "$identity" "$expected" "${actual:-missing}"
    status=1
  fi
}

if [[ ! -f "$package_resolved" ]]; then
  printf "err Package.resolved missing: %s\n" "$package_resolved"
  exit 1
fi

check_pin mlx-swift 0.31.3
check_pin mlx-swift-lm 3.31.3

exit "$status"
