#!/usr/bin/env bash
set -euo pipefail

PRODUCT="UltraTerm"
REPOSITORY="michael-berardi/ultraterm"
BUNDLE_ID="com.libertydesignstudio.ultraterm"
TEAM_ID="T63VT9UAY2"
ARCHIVE="UltraTerm-macos-arm64.zip"
INSTALL_SCOPE="user"
LAUNCH=1
INSTALL_DIR="${INSTALL_DIR:-}"
DOWNLOAD_BASE="${DOWNLOAD_BASE:-https://github.com/${REPOSITORY}/releases/latest/download}"
EXPECTED_VERSION="${EXPECTED_VERSION:-}"

usage() {
  cat <<'USAGE'
Install the latest prebuilt UltraTerm app.

Usage: install.sh [--user|--system] [--no-launch]

  --user       Install to ~/Applications (default; no sudo)
  --system     Install to /Applications (uses sudo)
  --no-launch  Do not open UltraTerm after installation

Environment overrides: DOWNLOAD_BASE, INSTALL_DIR.
UltraTerm requires the omp command at runtime. tmux is recommended for persistent sessions.
USAGE
}

verify_app_identity() {
  local app_path="$1"
  local details requirements identifier version
  [[ -d "$app_path" ]] || return 1
  identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
    "$app_path/Contents/Info.plist" 2>/dev/null || true)"
  [[ "$identifier" == "$BUNDLE_ID" ]] || return 1
  version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
    "$app_path/Contents/Info.plist" 2>/dev/null || true)"
  [[ "$version" == "$EXPECTED_VERSION" ]] || return 1
  /usr/bin/codesign --verify --deep --strict "$app_path" >/dev/null 2>&1 || return 1
  details="$(/usr/bin/codesign -dv --verbose=4 "$app_path" 2>&1)" || return 1
  /usr/bin/grep -Fqx -- "Identifier=${BUNDLE_ID}" <<<"$details" || return 1
  /usr/bin/grep -Fqx -- "TeamIdentifier=${TEAM_ID}" <<<"$details" || return 1
  /usr/bin/grep -Eq '^Authority=Developer ID Application: .+ \('"$TEAM_ID"'\)$' \
    <<<"$details" || return 1
  /usr/bin/grep -Ei 'flags=.*runtime' <<<"$details" >/dev/null || return 1
  requirements="$(/usr/bin/codesign -d -r- "$app_path" 2>&1)" || return 1
  /usr/bin/grep -Fq -- 'designated =>' <<<"$requirements" || return 1
  /usr/bin/grep -Fq -- "identifier \"${BUNDLE_ID}\"" <<<"$requirements" || return 1
  /usr/bin/grep -Fq -- 'anchor apple generic' <<<"$requirements" || return 1
  /usr/bin/grep -Eq -- "certificate .*OU.*${TEAM_ID}" <<<"$requirements" || return 1
}

launch_and_confirm_health() {
  local previous_pids new_pid candidate
  previous_pids="$(pgrep -f "${APP_TARGET}/Contents/MacOS/" || true)"
  open -n "$APP_TARGET" || return 1
  new_pid=""
  for _ in $(seq 1 50); do
    for candidate in $(pgrep -f "${APP_TARGET}/Contents/MacOS/" || true); do
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
  # Keep the verified backup until the new, previously unseen PID remains
  # alive for five seconds.
  for _ in $(seq 1 25); do
    kill -0 "$new_pid" 2>/dev/null || return 1
    sleep 0.2
  done
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --user) INSTALL_SCOPE="user" ;;
    --system) INSTALL_SCOPE="system" ;;
    --no-launch) LAUNCH=0 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "UltraTerm supports macOS only." >&2
  exit 1
fi
if [[ "$(uname -m)" != "arm64" ]]; then
  echo "UltraTerm currently supports Apple Silicon (arm64) only." >&2
  exit 1
fi

if [[ "$INSTALL_SCOPE" == "system" ]]; then
  INSTALL_DIR="${INSTALL_DIR:-/Applications}"
else
  INSTALL_DIR="${INSTALL_DIR:-${HOME}/Applications}"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ultraterm-install.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
ARCHIVE_PATH="${TMP_DIR}/${ARCHIVE}"
CHECKSUM_PATH="${ARCHIVE_PATH}.sha256"

if [[ -z "$EXPECTED_VERSION" ]]; then
  RELEASE_URL="$(curl --fail --silent --show-error --head "${DOWNLOAD_BASE}/${ARCHIVE}" |
    /usr/bin/awk 'tolower($1) == "location:" { sub(/\r$/, "", $2); print $2; exit }')"
  RELEASE_PATH="${RELEASE_URL#*/download/}"
  EXPECTED_VERSION="${RELEASE_PATH%%/*}"
  EXPECTED_VERSION="${EXPECTED_VERSION#v}"
fi
if [[ ! "$EXPECTED_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Unable to determine a stable release version from ${DOWNLOAD_BASE}/${ARCHIVE}." >&2
  exit 1
fi
curl --fail --location --silent --show-error "${DOWNLOAD_BASE}/${ARCHIVE}" --output "$ARCHIVE_PATH"
curl --fail --location --silent --show-error "${DOWNLOAD_BASE}/${ARCHIVE}.sha256" --output "$CHECKSUM_PATH"
(
  cd "$TMP_DIR"
  shasum -a 256 -c "${ARCHIVE}.sha256"
)

ditto -x -k "$ARCHIVE_PATH" "$TMP_DIR/unpacked"
APP_SOURCE="${TMP_DIR}/unpacked/UltraTerm-macos-arm64/${PRODUCT}.app"
if [[ ! -d "$APP_SOURCE" ]]; then
  echo "Release archive is missing UltraTerm.app." >&2
  exit 1
fi
if [[ ! -x "${APP_SOURCE}/Contents/Resources/omp-safe" ]]; then
  echo "Release archive is missing the bundled omp-safe launcher." >&2
  exit 1
fi
if ! verify_app_identity "$APP_SOURCE"; then
  echo "Release archive has an invalid UltraTerm Developer ID identity; refusing installation." >&2
  exit 1
fi
if ! spctl --assess --type execute "$APP_SOURCE" >/dev/null 2>&1; then
  echo "UltraTerm is not accepted by Gatekeeper; refusing installation." >&2
  exit 1
fi

APP_TARGET="${INSTALL_DIR}/${PRODUCT}.app"
APP_STAGE_ROOT="${INSTALL_DIR}/.${PRODUCT}.install.$$"
APP_STAGE="${APP_STAGE_ROOT}/${PRODUCT}.app"
APP_BACKUP="${INSTALL_DIR}/.${PRODUCT}.app.previous.$$"
if [[ "$INSTALL_SCOPE" == "system" ]]; then
  sudo mkdir -p "$INSTALL_DIR"
  sudo rm -rf "$APP_STAGE_ROOT" "$APP_BACKUP"
  sudo mkdir -p "$APP_STAGE_ROOT"
  sudo ditto "$APP_SOURCE" "$APP_STAGE"
  if ! verify_app_identity "$APP_STAGE"; then
    sudo rm -rf "$APP_STAGE_ROOT"
    echo "Staged UltraTerm bundle failed identity verification; refusing installation." >&2
    exit 1
  fi
  if [[ -d "$APP_TARGET" ]]; then
    sudo mv "$APP_TARGET" "$APP_BACKUP"
  fi
  if ! sudo mv "$APP_STAGE" "$APP_TARGET"; then
    [[ ! -d "$APP_BACKUP" ]] || sudo mv "$APP_BACKUP" "$APP_TARGET"
    exit 1
  fi
  sudo rm -rf "$APP_STAGE_ROOT"
else
  mkdir -p "$INSTALL_DIR"
  rm -rf "$APP_STAGE_ROOT" "$APP_BACKUP"
  mkdir -p "$APP_STAGE_ROOT"
  ditto "$APP_SOURCE" "$APP_STAGE"
  if ! verify_app_identity "$APP_STAGE"; then
    rm -rf "$APP_STAGE_ROOT"
    echo "Staged UltraTerm bundle failed identity verification; refusing installation." >&2
    exit 1
  fi
  if [[ -d "$APP_TARGET" ]]; then
    mv "$APP_TARGET" "$APP_BACKUP"
  fi
  if ! mv "$APP_STAGE" "$APP_TARGET"; then
    [[ ! -d "$APP_BACKUP" ]] || mv "$APP_BACKUP" "$APP_TARGET"
    exit 1
  fi
  rm -rf "$APP_STAGE_ROOT"
fi

echo "Installed UltraTerm to $APP_TARGET"
if ! command -v omp >/dev/null 2>&1; then
  echo "Warning: omp is not on PATH. Install OMP or configure OMP_BIN before opening terminals." >&2
fi
if ! command -v tmux >/dev/null 2>&1; then
  echo "Warning: tmux is not on PATH. UltraTerm can run OMP directly, but sessions will not persist." >&2
fi

restore_previous() {
  if [[ "$INSTALL_SCOPE" == "system" ]]; then
    sudo rm -rf "$APP_TARGET"
    if [[ -d "$APP_BACKUP" ]]; then
      sudo mv "$APP_BACKUP" "$APP_TARGET"
      open -n "$APP_TARGET" || true
    fi
  else
    rm -rf "$APP_TARGET"
    if [[ -d "$APP_BACKUP" ]]; then
      mv "$APP_BACKUP" "$APP_TARGET"
      open -n "$APP_TARGET" || true
    fi
  fi
}

if [[ "$LAUNCH" == "1" ]]; then
  if ! launch_and_confirm_health; then
    restore_previous
    echo "New UltraTerm failed PID/health confirmation; restored the previous copy." >&2
    exit 1
  fi
  if [[ "$INSTALL_SCOPE" == "system" ]]; then
    sudo rm -rf "$APP_BACKUP"
  else
    rm -rf "$APP_BACKUP"
  fi
else
  echo "UltraTerm was not launched; retaining rollback copy at $APP_BACKUP."
fi
