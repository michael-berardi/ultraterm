#!/usr/bin/env bash
set -euo pipefail

PRODUCT="UltraTerm"
REPOSITORY="michael-berardi/ultraterm"
ARCHIVE="UltraTerm-macos-arm64.zip"
INSTALL_SCOPE="user"
LAUNCH=1
INSTALL_DIR="${INSTALL_DIR:-}"
DOWNLOAD_BASE="${DOWNLOAD_BASE:-https://github.com/${REPOSITORY}/releases/latest/download}"
ALLOW_UNNOTARIZED="${ALLOW_UNNOTARIZED:-0}"

usage() {
  cat <<'USAGE'
Install the latest prebuilt UltraTerm app.

Usage: install.sh [--user|--system] [--no-launch]

  --user       Install to ~/Applications (default; no sudo)
  --system     Install to /Applications (uses sudo)
  --no-launch  Do not open UltraTerm after installation

Environment overrides: DOWNLOAD_BASE, INSTALL_DIR, ALLOW_UNNOTARIZED=1.
UltraTerm requires the omp command at runtime. tmux is recommended for persistent sessions.
USAGE
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
codesign --verify --deep --strict "$APP_SOURCE"
if ! spctl --assess --type execute "$APP_SOURCE" >/dev/null 2>&1 && [[ "$ALLOW_UNNOTARIZED" != "1" ]]; then
  echo "UltraTerm is not accepted by Gatekeeper; refusing installation." >&2
  exit 1
fi

APP_TARGET="${INSTALL_DIR}/${PRODUCT}.app"
APP_STAGE="${INSTALL_DIR}/.${PRODUCT}.app.install.$$"
APP_BACKUP="${INSTALL_DIR}/.${PRODUCT}.app.previous.$$"
if [[ "$INSTALL_SCOPE" == "system" ]]; then
  sudo mkdir -p "$INSTALL_DIR"
  sudo rm -rf "$APP_STAGE" "$APP_BACKUP"
  sudo ditto "$APP_SOURCE" "$APP_STAGE"
  codesign --verify --deep --strict "$APP_STAGE"
  if [[ -d "$APP_TARGET" ]]; then
    sudo mv "$APP_TARGET" "$APP_BACKUP"
  fi
  if ! sudo mv "$APP_STAGE" "$APP_TARGET"; then
    [[ ! -d "$APP_BACKUP" ]] || sudo mv "$APP_BACKUP" "$APP_TARGET"
    exit 1
  fi
  sudo rm -rf "$APP_BACKUP"
else
  mkdir -p "$INSTALL_DIR"
  rm -rf "$APP_STAGE" "$APP_BACKUP"
  ditto "$APP_SOURCE" "$APP_STAGE"
  codesign --verify --deep --strict "$APP_STAGE"
  if [[ -d "$APP_TARGET" ]]; then
    mv "$APP_TARGET" "$APP_BACKUP"
  fi
  if ! mv "$APP_STAGE" "$APP_TARGET"; then
    [[ ! -d "$APP_BACKUP" ]] || mv "$APP_BACKUP" "$APP_TARGET"
    exit 1
  fi
  rm -rf "$APP_BACKUP"
fi
codesign --verify --deep --strict "$APP_TARGET"

echo "Installed UltraTerm to $APP_TARGET"
if ! command -v omp >/dev/null 2>&1; then
  echo "Warning: omp is not on PATH. Install OMP or configure OMP_BIN before opening terminals." >&2
fi
if ! command -v tmux >/dev/null 2>&1; then
  echo "Warning: tmux is not on PATH. UltraTerm can run OMP directly, but sessions will not persist." >&2
fi
if [[ "$LAUNCH" == "1" ]]; then
  open "$APP_TARGET"
fi
