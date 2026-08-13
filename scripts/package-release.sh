#!/usr/bin/env bash
set -euo pipefail

export PATH="${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:${PATH}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT="UltraTerm"
VERSION="${VERSION:-$(node -p "require('${ROOT_DIR}/package.json').version")}"
ARCH="${PACKAGE_ARCH:-$(uname -m)}"
OUTPUT_DIR="${OUTPUT_DIR:-${ROOT_DIR}/release}"
PAYLOAD_NAME="${PRODUCT}-macos-${ARCH}"
ARCHIVE_NAME="${PAYLOAD_NAME}.zip"
APP_PATH="${APP_PATH:-${ROOT_DIR}/src-tauri/target/release/bundle/macos/${PRODUCT}.app}"
BUILD_QUEUE="${BUILD_QUEUE:-/Users/libertydesignstudio/dev/scripts/build_queue.py}"
ALLOW_ADHOC="${ALLOW_ADHOC:-0}"

if [[ "$ALLOW_ADHOC" != "1" ]]; then
  APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Michael Berardi (T63VT9UAY2)}"
  APPLE_INSTALLER_SIGNING_IDENTITY="${APPLE_INSTALLER_SIGNING_IDENTITY:-Developer ID Installer: Michael Berardi (T63VT9UAY2)}"
  NOTARYTOOL_PROFILE="${NOTARYTOOL_PROFILE:-ImploseLabs-Notarization}"
fi

if [[ "$(uname -s)" != "Darwin" || "$ARCH" != "arm64" ]]; then
  echo "Release packaging currently requires Apple Silicon macOS." >&2
  exit 1
fi

SIGNED_RELEASE=0
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  : "${APPLE_INSTALLER_SIGNING_IDENTITY:?APPLE_INSTALLER_SIGNING_IDENTITY is required}"
  : "${NOTARYTOOL_PROFILE:?NOTARYTOOL_PROFILE is required}"
  SIGNED_RELEASE=1
elif [[ "$ALLOW_ADHOC" != "1" ]]; then
  echo "APPLE_SIGNING_IDENTITY is required. Use ALLOW_ADHOC=1 only for local installer testing." >&2
  exit 1
fi

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  if [[ -f "$BUILD_QUEUE" ]]; then
    (cd "$ROOT_DIR" && python3 "$BUILD_QUEUE" --project "$ROOT_DIR" -- env CI=false npm run tauri -- build --bundles app)
  else
    (cd "$ROOT_DIR" && CI=false npm run tauri -- build --bundles app)
  fi
fi
if [[ ! -d "$APP_PATH" ]]; then
  echo "Missing UltraTerm.app release bundle." >&2
  exit 1
fi
if [[ ! -x "${APP_PATH}/Contents/Resources/omp-safe" ]]; then
  echo "UltraTerm.app is missing its bundled omp-safe launcher." >&2
  exit 1
fi

VERSIONED_PKG="${OUTPUT_DIR}/${PRODUCT}-${VERSION}-${ARCH}.pkg"
STABLE_PKG="${OUTPUT_DIR}/${PRODUCT}-macos-${ARCH}.pkg"
ARCHIVE_PATH="${OUTPUT_DIR}/${ARCHIVE_NAME}"
mkdir -p "$OUTPUT_DIR"
rm -f "$VERSIONED_PKG" "${VERSIONED_PKG}.sha256" \
  "$STABLE_PKG" "${STABLE_PKG}.sha256" \
  "$ARCHIVE_PATH" "${ARCHIVE_PATH}.sha256"
if [[ "$SIGNED_RELEASE" == "1" ]]; then
  SKIP_BUILD=1 OUTPUT_DIR="$OUTPUT_DIR" REQUIRE_SIGNED=1 NOTARIZE=1 \
    APPLE_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY" \
    APPLE_INSTALLER_SIGNING_IDENTITY="$APPLE_INSTALLER_SIGNING_IDENTITY" \
    NOTARYTOOL_PROFILE="$NOTARYTOOL_PROFILE" \
    "${ROOT_DIR}/scripts/build-pkg.sh"
else
  SKIP_BUILD=1 OUTPUT_DIR="$OUTPUT_DIR" "${ROOT_DIR}/scripts/build-pkg.sh"
fi

mv "$VERSIONED_PKG" "$STABLE_PKG"
rm -f "${VERSIONED_PKG}.sha256"

STAGING_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ultraterm-release.XXXXXX")"
trap 'rm -rf "$STAGING_DIR"' EXIT
PAYLOAD_DIR="${STAGING_DIR}/${PAYLOAD_NAME}"
mkdir -p "$PAYLOAD_DIR"
ditto "$APP_PATH" "${PAYLOAD_DIR}/${PRODUCT}.app"

ditto -c -k --sequesterRsrc --keepParent "$PAYLOAD_DIR" "$ARCHIVE_PATH"
if [[ "$SIGNED_RELEASE" == "1" ]]; then
  xcrun notarytool submit "$ARCHIVE_PATH" --keychain-profile "$NOTARYTOOL_PROFILE" --wait
  xcrun stapler staple "${PAYLOAD_DIR}/${PRODUCT}.app"
  rm -f "$ARCHIVE_PATH"
  ditto -c -k --sequesterRsrc --keepParent "$PAYLOAD_DIR" "$ARCHIVE_PATH"
fi

codesign --verify --deep --strict "${PAYLOAD_DIR}/${PRODUCT}.app"
(
  cd "$OUTPUT_DIR"
  shasum -a 256 "$(basename "$STABLE_PKG")" > "$(basename "$STABLE_PKG").sha256"
  shasum -a 256 "$ARCHIVE_NAME" > "${ARCHIVE_NAME}.sha256"
)
printf 'Release assets staged in %s\n' "$OUTPUT_DIR"
