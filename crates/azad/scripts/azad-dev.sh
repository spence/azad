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
  AZAD_APP_DIR \
  AZAD_BUILD_PROFILE \
  AZAD_TOON_SHOW_PARTIALS \
  AZAD_NATIVE_ENGINE_LOGS \
  AZAD_CODESIGN_IDENTITY; do
  capture_env_var "$var"
done

CONFIG_FILE="${AZAD_CONFIG:-$ROOT_DIR/.codesign.env}"
if [[ -f "$CONFIG_FILE" ]]; then
  # Local, ignored shell assignments for developer-specific install settings.
  source "$CONFIG_FILE"
fi

for var in \
  AZAD_APP_DIR \
  AZAD_BUILD_PROFILE \
  AZAD_TOON_SHOW_PARTIALS \
    AZAD_NATIVE_ENGINE_LOGS \
  AZAD_CODESIGN_IDENTITY; do
  restore_env_var "$var"
done

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
TOON_SHOW_PARTIALS="${AZAD_TOON_SHOW_PARTIALS:-0}"
AZAD_NATIVE_ENGINE_LOGS="${AZAD_NATIVE_ENGINE_LOGS:-0}"
APP_SIGNED=0
APP_STABLE_SIGNED=0

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

  local target_dir="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
  if [[ -n "${CARGO_TARGET_DIR:-}" && "$target_dir" != /* ]]; then
    target_dir="${CRATE_DIR}/${target_dir}"
  fi

  local profile_dir
  if [[ "$BUILD_PROFILE" == "release" ]]; then
    profile_dir="release"
  else
    profile_dir="debug"
  fi

  BIN_SOURCE="${target_dir}/${profile_dir}/azad"
  if [[ ! -x "$BIN_SOURCE" ]]; then
    echo "error: built binary missing at $BIN_SOURCE" >&2
    exit 1
  fi
}

codesign_app_if_configured() {
  if [[ -z "${AZAD_CODESIGN_IDENTITY:-}" ]]; then
    rm -rf "${APP_CONTENTS_DIR}/_CodeSignature"
    echo "Code signing: disabled (no AZAD_CODESIGN_IDENTITY in ${CONFIG_FILE})"
    return
  fi

  if [[ ! -x /usr/bin/codesign ]]; then
    echo "error: /usr/bin/codesign not available; cannot sign with AZAD_CODESIGN_IDENTITY" >&2
    exit 1
  fi

  local err_file
  err_file="$(mktemp "${TMPDIR:-/tmp}/azad-codesign.XXXXXX")"
  if /usr/bin/codesign \
    --force \
    --deep \
    --sign "$AZAD_CODESIGN_IDENTITY" \
    --entitlements "$CRATE_DIR/Azad.entitlements" \
    "$APP_DIR" 2>"$err_file"; then
    rm -f "$err_file"
    APP_SIGNED=1
    if [[ "$AZAD_CODESIGN_IDENTITY" != "-" ]]; then
      APP_STABLE_SIGNED=1
    fi
    echo "Signed Azad.app with identity: $AZAD_CODESIGN_IDENTITY"
    return
  fi

  echo "error: codesign failed with identity: $AZAD_CODESIGN_IDENTITY" >&2
  sed 's/^/  /' "$err_file" >&2
  rm -f "$err_file"
  exit 1
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
  install -m 644 "${CRATE_DIR}/assets/claude.svg" "${APP_RESOURCES_DIR}/claude.svg"
  write_info_plist
  write_launch_agent_plist
  codesign_app_if_configured

  echo "Installed Azad app bundle at: $APP_DIR"
  echo "Installed LaunchAgent plist at: $PLIST_PATH"
  if [[ "$APP_STABLE_SIGNED" == "1" ]]; then
    echo "Permissions are preserved across install/restart unless you run: just reset-permissions"
  elif [[ "$APP_SIGNED" == "1" ]]; then
    echo "Installed ad-hoc signed development build. Stable local signing is optional; see .codesign.env.example."
  else
    echo "Installed unsigned development build. Local signing is optional; see .codesign.env.example."
  fi
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
