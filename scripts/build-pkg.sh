#!/bin/bash
set -euo pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:${PATH}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT_NAME="UltraTerm"
BUNDLE_ID="com.libertydesignstudio.ultraterm"
VERSION="${VERSION:-$(node -p "require('${ROOT_DIR}/package.json').version")}"
ARCH="${PACKAGE_ARCH:-$(uname -m)}"
OUTPUT_DIR="${OUTPUT_DIR:-${ROOT_DIR}/release}"
PKG_PATH="${OUTPUT_DIR}/${PRODUCT_NAME}-${VERSION}-${ARCH}.pkg"

if [[ "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  TAG_VERSION="${GITHUB_REF_NAME#v}"
  if [[ "$TAG_VERSION" != "$VERSION" ]]; then
    echo "Tag version ${TAG_VERSION} does not match package version ${VERSION}." >&2
    exit 1
  fi
fi

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  BUILD_ARGS=(build --bundles app)
  if [[ -n "${TAURI_TARGET:-}" ]]; then
    BUILD_ARGS+=(--target "$TAURI_TARGET")
  fi
  (cd "$ROOT_DIR" && CI=false npm run tauri -- "${BUILD_ARGS[@]}")
fi

TARGET_SEGMENT="${TAURI_TARGET:+${TAURI_TARGET}/}release"
APP_CANDIDATES=(
  "${ROOT_DIR}/src-tauri/target/${TARGET_SEGMENT}/bundle/macos/${PRODUCT_NAME}.app"
  "${ROOT_DIR}/target/${TARGET_SEGMENT}/bundle/macos/${PRODUCT_NAME}.app"
)
APP_PATH="${APP_PATH:-}"
if [[ -z "$APP_PATH" ]]; then
  for candidate in "${APP_CANDIDATES[@]}"; do
    if [[ -d "$candidate" ]]; then
      APP_PATH="$candidate"
      break
    fi
  done
fi
if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  printf 'Could not locate %s.app. Checked:\n' "$PRODUCT_NAME" >&2
  printf '  %s\n' "${APP_CANDIDATES[@]}" >&2
  exit 1
fi

ALLOW_ADHOC="${ALLOW_ADHOC:-0}"
if [[ "${REQUIRE_SIGNED:-0}" == "1" && -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "APPLE_SIGNING_IDENTITY is required for a distributable package." >&2
  exit 1
fi
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$APP_PATH"
elif [[ "$ALLOW_ADHOC" == "1" ]]; then
  codesign --force --deep --sign - "$APP_PATH"
else
  echo "APPLE_SIGNING_IDENTITY is required. Use ALLOW_ADHOC=1 only for local package testing." >&2
  exit 1
fi
if [[ "$ALLOW_ADHOC" == "1" ]]; then
  EXPECTED_VERSION="$VERSION" \
    "${ROOT_DIR}/scripts/verify-app-identity.sh" --allow-adhoc "$APP_PATH"
else
  EXPECTED_VERSION="$VERSION" "${ROOT_DIR}/scripts/verify-app-identity.sh" "$APP_PATH"
fi

mkdir -p "$OUTPUT_DIR"
rm -f "$PKG_PATH"
PKGBUILD_ARGS=(
  --component "$APP_PATH"
  --install-location /Applications
  --identifier "$BUNDLE_ID"
  --version "$VERSION"
)
if [[ "${REQUIRE_SIGNED:-0}" == "1" && -z "${APPLE_INSTALLER_SIGNING_IDENTITY:-}" ]]; then
  echo "APPLE_INSTALLER_SIGNING_IDENTITY is required for a distributable package." >&2
  exit 1
fi
if [[ -n "${APPLE_INSTALLER_SIGNING_IDENTITY:-}" ]]; then
  PKGBUILD_ARGS+=(--sign "$APPLE_INSTALLER_SIGNING_IDENTITY")
fi
pkgbuild "${PKGBUILD_ARGS[@]}" "$PKG_PATH"

if [[ "${NOTARIZE:-0}" == "1" ]]; then
  if [[ -n "${NOTARYTOOL_PROFILE:-}" ]]; then
    xcrun notarytool submit "$PKG_PATH" --keychain-profile "$NOTARYTOOL_PROFILE" --wait
  elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    xcrun notarytool submit "$PKG_PATH" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
  else
    echo "Notarization requires NOTARYTOOL_PROFILE or APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID." >&2
    exit 1
  fi
  xcrun stapler staple "$PKG_PATH"
fi

EXPECTED_VERSION="$VERSION" REQUIRE_SIGNED="${REQUIRE_SIGNED:-0}" \
  REQUIRE_NOTARIZED="${NOTARIZE:-0}" ALLOW_ADHOC="$ALLOW_ADHOC" \
  "${ROOT_DIR}/scripts/verify-pkg.sh" "$PKG_PATH"
shasum -a 256 "$PKG_PATH" > "${PKG_PATH}.sha256"
printf '%s\n' "$PKG_PATH"
