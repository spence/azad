#!/usr/bin/env bash
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT_DIR="$(cd "${CRATE_DIR}/../.." && pwd)"
LABEL="ai.azad"
LEGACY_LABEL="com.spence.azad"
DOMAIN="gui/$(id -u)"
SERVICE_TARGET="${DOMAIN}/${LABEL}"
LEGACY_SERVICE_TARGET="${DOMAIN}/${LEGACY_LABEL}"

APP_DIR="${AZAD_APP_DIR:-$HOME/Applications/Azad.app}"
APP_CONTENTS_DIR="${APP_DIR}/Contents"
APP_MACOS_DIR="${APP_CONTENTS_DIR}/MacOS"
APP_RESOURCES_DIR="${APP_CONTENTS_DIR}/Resources"

LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
PLIST_PATH="${LAUNCH_AGENTS_DIR}/${LABEL}.plist"
LEGACY_PLIST_PATH="${LAUNCH_AGENTS_DIR}/${LEGACY_LABEL}.plist"

LOG_DIR="$HOME/Library/Logs/Azad"
STDOUT_LOG="${LOG_DIR}/stdout.log"
STDERR_LOG="${LOG_DIR}/stderr.log"

BUILD_PROFILE="${AZAD_BUILD_PROFILE:-release}"
if [[ "$BUILD_PROFILE" == "release" ]]; then
  BIN_SOURCE="${ROOT_DIR}/target/release/azad"
else
  BIN_SOURCE="${ROOT_DIR}/target/debug/azad"
fi

TOON_SHOW_PARTIALS="${AZAD_TOON_SHOW_PARTIALS:-0}"
AZAD_NATIVE_ENGINE_LOGS="${AZAD_NATIVE_ENGINE_LOGS:-0}"

CODESIGN_IDENTITY="${AZAD_CODESIGN_IDENTITY:-}"
if [[ -z "$CODESIGN_IDENTITY" ]] && [[ -x /usr/bin/security ]]; then
  # TCC (Accessibility/Microphone/Input Monitoring) keys permission grants off the
  # app's code-signing identity. An ad-hoc signature has no stable identity, so TCC
  # falls back to the binary's cdhash — which changes on every rebuild, forcing the
  # user to re-grant permissions after each `just install`. Sign with a stable
  # identity so the designated requirement (and thus the grants) persist across builds.
  #
  # Preference order: an explicit Azad dev root (purpose-made), then the longer-lived
  # Developer ID, then Apple Development. Falls through to ad-hoc only if none exist.
  #
  # We capture the 40-hex SHA-1 hash (field 2 of `1) <hash> "<name>"`), not the name,
  # and sign with that. A machine that imported an exported identity AND has its own
  # cert from the same team ends up with multiple certs sharing one common name, and
  # `codesign --sign "<name>"` then fails as "ambiguous". The hash is unique per cert.
  IDENTITY_LIST="$(/usr/bin/security find-identity -v -p codesigning \
    "$HOME/Library/Keychains/login.keychain-db" 2>/dev/null)"
  for pattern in "Azad Dev Code Signing Root" "Developer ID Application" "Apple Development"; do
    DETECTED_IDENTITY="$(printf '%s\n' "$IDENTITY_LIST" \
      | awk -v p="$pattern" 'index($0, p){print $2; exit}')"
    if [[ -n "${DETECTED_IDENTITY:-}" ]]; then
      CODESIGN_IDENTITY="$DETECTED_IDENTITY"
      break
    fi
  done
fi

usage() {
  cat <<'USAGE'
Usage: scripts/azad-dev.sh <command>

Commands:
  install      Build Azad and install/update ~/Applications/Azad.app + LaunchAgent plist
  start        Start (or restart) Azad via launchctl
  stop         Stop Azad via launchctl
  restart      Stop then start Azad via launchctl
  status       Print launchctl status for Azad
  logs         Tail Azad stdout/stderr logs
  reset-permissions  Reset macOS TCC permissions for Azad (Microphone + Accessibility)
  uninstall    Remove LaunchAgent plist and stop service (keeps app bundle)
USAGE
}

build_binary() {
  pushd "$CRATE_DIR" >/dev/null
  if [[ "$BUILD_PROFILE" == "release" ]]; then
    cargo build --release
  else
    cargo build
  fi
  popd >/dev/null

  if [[ ! -x "$BIN_SOURCE" ]]; then
    echo "error: built binary missing at $BIN_SOURCE" >&2
    exit 1
  fi
}

codesign_app_if_configured() {
  if [[ -z "$CODESIGN_IDENTITY" ]]; then
    echo "Note: AZAD_CODESIGN_IDENTITY is not set; Accessibility permission may need re-approval after updates."
    return
  fi

  if [[ ! -x /usr/bin/codesign ]]; then
    echo "warning: /usr/bin/codesign not available; skipping app signing" >&2
    return
  fi

  /usr/bin/codesign \
    --force \
    --deep \
    --sign "$CODESIGN_IDENTITY" \
    --entitlements "$CRATE_DIR/Azad.entitlements" \
    "$APP_DIR"

  echo "Signed Azad.app with identity: $CODESIGN_IDENTITY"
}

write_info_plist() {
  cat >"${APP_CONTENTS_DIR}/Info.plist" <<EOF
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
  <string>0.1.0</string>
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
}

write_launch_agent_plist() {
  mkdir -p "$LAUNCH_AGENTS_DIR" "$LOG_DIR"

  cat >"$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${APP_MACOS_DIR}/azad</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>LimitLoadToSessionType</key>
  <array>
    <string>Aqua</string>
  </array>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>Nice</key>
  <integer>-10</integer>
  <key>EnvironmentVariables</key>
  <dict>
    <key>AZAD_ASSETS_DIR</key>
    <string>${APP_RESOURCES_DIR}</string>
    <key>TOON_SHOW_PARTIALS</key>
    <string>${TOON_SHOW_PARTIALS}</string>
    <key>AZAD_NATIVE_ENGINE_LOGS</key>
    <string>${AZAD_NATIVE_ENGINE_LOGS}</string>
  </dict>
  <key>StandardOutPath</key>
  <string>${STDOUT_LOG}</string>
  <key>StandardErrorPath</key>
  <string>${STDERR_LOG}</string>
</dict>
</plist>
EOF
}

is_loaded() {
  launchctl print "$SERVICE_TARGET" >/dev/null 2>&1
}

legacy_is_loaded() {
  launchctl print "$LEGACY_SERVICE_TARGET" >/dev/null 2>&1
}

cleanup_legacy_service() {
  if [[ "$LEGACY_LABEL" == "$LABEL" ]]; then
    return
  fi

  if legacy_is_loaded; then
    launchctl bootout "$LEGACY_SERVICE_TARGET" >/dev/null 2>&1 || true
  fi

  if [[ -f "$LEGACY_PLIST_PATH" ]]; then
    rm -f "$LEGACY_PLIST_PATH"
  fi
}

bootstrap_until_loaded() {
  local attempt
  for attempt in $(seq 1 20); do
    launchctl bootstrap "$DOMAIN" "$PLIST_PATH" >/dev/null 2>&1 || true
    if is_loaded; then
      return 0
    fi

    sleep 0.2
  done

  if is_loaded; then
    return 0
  fi

  echo "error: unable to load launchd service ${SERVICE_TARGET}" >&2
  return 1
}

kickstart_with_retry() {
  local attempt
  for attempt in $(seq 1 20); do
    if launchctl kickstart -k "$SERVICE_TARGET" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done
  return 1
}

cmd_install() {
  cleanup_legacy_service
  build_binary

  mkdir -p "$APP_MACOS_DIR" "$APP_RESOURCES_DIR"
  install -m 755 "$BIN_SOURCE" "${APP_MACOS_DIR}/azad"
  install -m 644 "${CRATE_DIR}/assets/azad-black.png" "${APP_RESOURCES_DIR}/azad-black.png"
  install -m 644 "${CRATE_DIR}/assets/azad-white.png" "${APP_RESOURCES_DIR}/azad-white.png"
  install -m 644 "${CRATE_DIR}/assets/azad.icns" "${APP_RESOURCES_DIR}/azad.icns"
  write_info_plist
  write_launch_agent_plist
  codesign_app_if_configured

  echo "Installed Azad app bundle at: $APP_DIR"
  echo "Installed LaunchAgent plist at: $PLIST_PATH"
  echo "Permissions are preserved across install/restart unless you run: just reset-permissions"
  echo "Next: just start"
}

cmd_start() {
  cleanup_legacy_service
  if [[ ! -f "$PLIST_PATH" ]]; then
    echo "error: LaunchAgent plist not found at $PLIST_PATH (run just install first)" >&2
    exit 1
  fi

  bootstrap_until_loaded

  if ! kickstart_with_retry; then
    # If kickstart failed (e.g., stale state after rapid stop/start), force a clean reload.
    launchctl bootout "$SERVICE_TARGET" >/dev/null 2>&1 || true
    bootstrap_until_loaded
    kickstart_with_retry
  fi

  echo "Started launchd service: $SERVICE_TARGET"
}

cmd_stop() {
  if is_loaded; then
    if ! launchctl bootout "$SERVICE_TARGET" >/dev/null 2>&1; then
      launchctl bootout "$DOMAIN" "$PLIST_PATH" >/dev/null 2>&1 || true
    fi
    echo "Stopped launchd service: $SERVICE_TARGET"
  else
    echo "Service not loaded: $SERVICE_TARGET"
  fi

  # Also kill any standalone instance launchctl doesn't know about. If the
  # user Cmd+Q'd and reopened Azad from Spotlight (or any path other than
  # `just start`), the running process isn't a child of the LaunchAgent —
  # bootout leaves it alive and the next kickstart bails through the app's
  # secondary-launch-focuses-existing-instance guard, leaving a stale
  # binary loaded. Match by the bundle's MacOS path so we never touch
  # anything outside this app bundle.
  local bundle_bin="${APP_MACOS_DIR}/azad"
  if pgrep -f "${bundle_bin}" >/dev/null 2>&1; then
    pkill -TERM -f "${bundle_bin}" >/dev/null 2>&1 || true
    local _i
    for _i in $(seq 1 20); do
      pgrep -f "${bundle_bin}" >/dev/null 2>&1 || break
      sleep 0.1
    done
    pkill -KILL -f "${bundle_bin}" >/dev/null 2>&1 || true
    echo "Stopped standalone Azad processes matching: ${bundle_bin}"
  fi
}

cmd_restart() {
  cmd_stop
  cmd_start
}

cmd_status() {
  if is_loaded; then
    launchctl print "$SERVICE_TARGET"
  else
    echo "Service not loaded: $SERVICE_TARGET"
    exit 1
  fi
}

cmd_logs() {
  mkdir -p "$LOG_DIR"
  touch "$STDOUT_LOG" "$STDERR_LOG"
  tail -f "$STDOUT_LOG" "$STDERR_LOG"
}

cmd_reset_permissions() {
  /usr/bin/tccutil reset Microphone "$LABEL" || true
  /usr/bin/tccutil reset Accessibility "$LABEL" || true
  echo "Reset TCC permissions for $LABEL (Microphone + Accessibility)."
  echo "The next app launch can trigger permission prompts again."
}

cmd_uninstall() {
  cmd_stop || true
  rm -f "$PLIST_PATH"
  echo "Removed LaunchAgent plist: $PLIST_PATH"
  echo "App bundle preserved at: $APP_DIR"
}

main() {
  local cmd="${1:-}"
  case "$cmd" in
    install) cmd_install ;;
    start) cmd_start ;;
    stop) cmd_stop ;;
    restart) cmd_restart ;;
    status) cmd_status ;;
    logs) cmd_logs ;;
    reset-permissions) cmd_reset_permissions ;;
    uninstall) cmd_uninstall ;;
    ""|-h|--help|help)
      usage
      ;;
    *)
      echo "error: unknown command '$cmd'" >&2
      usage
      exit 1
      ;;
  esac
}

main "${1:-}"
