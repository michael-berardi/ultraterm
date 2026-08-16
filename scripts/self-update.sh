#!/usr/bin/env bash
# self-update.sh — build UltraTerm, swap it into /Applications, and restart it
# in place. No installer package: the app bundle is replaced directly.
#
# Terminal sessions survive the whole cycle: UltraTerm detaches its PTY
# clients on exit while the tmux-backed OMP sessions keep running, and the new
# instance reattaches every live slot during bootstrap.
#
# Usage:
#   scripts/self-update.sh              build, install, restart
#   scripts/self-update.sh --restart    restart only (install current build)
#   SKIP_INSTALL=1 scripts/self-update.sh   build only, no install/restart
set -euo pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:${PATH}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT_NAME="UltraTerm"
APP_IDENT="ultraterm"
INSTALL_PATH="${INSTALL_PATH:-/Applications/${PRODUCT_NAME}.app}"
BACKUP_DIR="${ROOT_DIR}/.app-backup"
SKIP_BUILD=0

for arg in "$@"; do
  case "$arg" in
    --restart) SKIP_BUILD=1 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 2
      ;;
  esac
done

BUILD_ARGS=(build --bundles app)
BUILD_CMD=(npm run tauri -- "${BUILD_ARGS[@]}")
BUILD_QUEUE="${BUILD_QUEUE:-$HOME/dev/scripts/build_queue.py}"
# The tauri CLI maps the CI env var onto its --ci flag and rejects CI=1.
export CI=false

launch_and_confirm_health() {
  local app_path="$1" previous_pids new_pid candidate
  previous_pids="$(pgrep -f "${app_path}/Contents/MacOS/" || true)"
  open -n "$app_path" || return 1
  new_pid=""
  for _ in $(seq 1 50); do
    for candidate in $(pgrep -f "${app_path}/Contents/MacOS/" || true); do
      [[ "$candidate" == "$$" ]] && continue
      case " ${previous_pids} " in
        *" ${candidate} "*) ;;
        *) new_pid="$candidate"; break ;;
      esac
    done
    [[ -n "$new_pid" ]] && break
    sleep 0.2
  done
  [[ -n "$new_pid" ]] || return 1
  for _ in $(seq 1 25); do
    kill -0 "$new_pid" 2>/dev/null || return 1
    sleep 0.2
  done
}

if [[ "$SKIP_BUILD" != "1" ]]; then
  if [[ -x "$(command -v python3 || true)" && -f "$BUILD_QUEUE" ]]; then
    python3 "$BUILD_QUEUE" --project "$ROOT_DIR" -- "${BUILD_CMD[@]}"
  else
    (cd "$ROOT_DIR" && "${BUILD_CMD[@]}")
  fi
fi

BUILT_APP="${ROOT_DIR}/src-tauri/target/release/bundle/macos/${PRODUCT_NAME}.app"
if [[ ! -d "$BUILT_APP" ]]; then
  echo "Build artifact missing: $BUILT_APP" >&2
  exit 1
fi

if [[ "${SKIP_INSTALL:-0}" == "1" ]]; then
  echo "SKIP_INSTALL=1 — build complete at $BUILT_APP"
  exit 0
fi

# Sign local installs with the stable Developer ID identity (same as release
# builds) so macOS permissions granted to UltraTerm — Screen Recording,
# Accessibility, etc. — survive every update. Ad-hoc signing (`--sign -`)
# creates a new code identity per build and silently invalidates those grants;
# use ALLOW_ADHOC=1 only for throwaway test builds.
if [[ "${ALLOW_ADHOC:-0}" == "1" ]]; then
  codesign --force --deep --sign - "$BUILT_APP"
else
  SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Michael Berardi (T63VT9UAY2)}"
  codesign --force --deep --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$BUILT_APP"
fi
if [[ "${ALLOW_ADHOC:-0}" == "1" ]]; then
  EXPECTED_VERSION="$(node -p "require('${ROOT_DIR}/package.json').version")" \
    "${ROOT_DIR}/scripts/verify-app-identity.sh" --allow-adhoc "$BUILT_APP"
else
  EXPECTED_VERSION="$(node -p "require('${ROOT_DIR}/package.json').version")" \
    "${ROOT_DIR}/scripts/verify-app-identity.sh" "$BUILT_APP"
fi

APP_STAGE_ROOT="${INSTALL_PATH}.install.$$"
APP_STAGE="${APP_STAGE_ROOT}/${PRODUCT_NAME}.app"
APP_SWAP_BACKUP="${INSTALL_PATH}.previous.$$"
rm -rf "$APP_STAGE_ROOT" "$APP_SWAP_BACKUP"
mkdir -p "$APP_STAGE_ROOT"
ditto "$BUILT_APP" "$APP_STAGE"
if [[ "${ALLOW_ADHOC:-0}" == "1" ]]; then
  EXPECTED_VERSION="$(node -p "require('${ROOT_DIR}/package.json').version")" \
    "${ROOT_DIR}/scripts/verify-app-identity.sh" --allow-adhoc "$APP_STAGE"
else
  EXPECTED_VERSION="$(node -p "require('${ROOT_DIR}/package.json').version")" \
    "${ROOT_DIR}/scripts/verify-app-identity.sh" "$APP_STAGE"
fi

if [[ -d "$INSTALL_PATH" ]]; then
  mkdir -p "$BACKUP_DIR"
  BACKUP_PATH="${BACKUP_DIR}/${PRODUCT_NAME}-$(date +%Y%m%d-%H%M%S).app"
  echo "Backing up current install to $BACKUP_PATH"
  ditto "$INSTALL_PATH" "$BACKUP_PATH"
fi

# Graceful quit first: the app's exit handler detaches terminal clients so
# their tmux sessions (and all OMP work inside them) keep running.
RUNNING=0
if pgrep -f "${INSTALL_PATH}/Contents/MacOS" >/dev/null 2>&1; then
  RUNNING=1
  osascript -e "tell application \"${PRODUCT_NAME}\" to quit" >/dev/null 2>&1 || true
  for _ in $(seq 1 50); do
    pgrep -f "${INSTALL_PATH}/Contents/MacOS" >/dev/null 2>&1 || break
    sleep 0.2
  done
  if pgrep -f "${INSTALL_PATH}/Contents/MacOS" >/dev/null 2>&1; then
    echo "UltraTerm did not exit within 10s; refusing to swap a running bundle." >&2
    exit 1
  fi
fi

if [[ -d "$INSTALL_PATH" ]]; then
  if ! mv "$INSTALL_PATH" "$APP_SWAP_BACKUP"; then
    echo "Unable to move the current UltraTerm bundle aside; refusing to swap." >&2
    exit 1
  fi
fi
if ! mv "$APP_STAGE" "$INSTALL_PATH"; then
  [[ ! -d "$APP_SWAP_BACKUP" ]] || mv "$APP_SWAP_BACKUP" "$INSTALL_PATH"
  echo "Unable to install the verified UltraTerm bundle; restored the previous copy." >&2
  exit 1
fi
rm -rf "$APP_STAGE_ROOT"
echo "Installed ${PRODUCT_NAME} to $INSTALL_PATH"

restore_previous() {
  rm -rf "$INSTALL_PATH"
  if [[ -d "$APP_SWAP_BACKUP" ]]; then
    mv "$APP_SWAP_BACKUP" "$INSTALL_PATH"
    open -n "$INSTALL_PATH" || true
  fi
}

if [[ "$RUNNING" == "1" || "${ALWAYS_RELAUNCH:-0}" == "1" ]]; then
  if ! launch_and_confirm_health "$INSTALL_PATH"; then
    restore_previous
    echo "New UltraTerm failed PID/health confirmation; restored the previous copy." >&2
    exit 1
  fi
  rm -rf "$APP_SWAP_BACKUP"
  echo "Relaunched ${PRODUCT_NAME}; new PID remained healthy for five seconds."
else
  echo "UltraTerm was not running; rollback copy retained at $APP_SWAP_BACKUP."
  echo "Launch it with: open -n \"$INSTALL_PATH\""
fi
