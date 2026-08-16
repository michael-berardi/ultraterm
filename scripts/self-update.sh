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
  codesign --force --deep --sign "$SIGNING_IDENTITY" "$BUILT_APP"
fi
codesign --verify --deep --strict "$BUILT_APP"

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

rm -rf "$INSTALL_PATH"
ditto "$BUILT_APP" "$INSTALL_PATH"
xattr -dr com.apple.quarantine "$INSTALL_PATH" 2>/dev/null || true
echo "Installed ${PRODUCT_NAME} to $INSTALL_PATH"

if [[ "$RUNNING" == "1" || "${ALWAYS_RELAUNCH:-0}" == "1" ]]; then
  open -n "$INSTALL_PATH"
  echo "Relaunched ${PRODUCT_NAME}; terminal sessions reattach automatically."
else
  echo "UltraTerm was not running; launch it with: open -n \"$INSTALL_PATH\""
fi
