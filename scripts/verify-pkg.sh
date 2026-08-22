#!/bin/bash
set -euo pipefail
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:${PATH}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PRODUCT_NAME="UltraTerm"
BUNDLE_ID="com.libertydesignstudio.ultraterm"
ALLOW_ADHOC="${ALLOW_ADHOC:-0}"
EXPECTED_VERSION="${EXPECTED_VERSION:-}"
PKG_PATH="${1:-}"
INSTALL_MODE="${2:-}"
if [[ -z "$PKG_PATH" || ! -f "$PKG_PATH" ]]; then
  echo "Usage: $0 /path/to/UltraTerm.pkg [--install]" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
pkgutil --expand-full "$PKG_PATH" "$TMP_DIR/expanded"
PAYLOAD_LIST="$TMP_DIR/payload-files.txt"
pkgutil --payload-files "$PKG_PATH" > "$PAYLOAD_LIST"

PACKAGE_INFO="$TMP_DIR/expanded/PackageInfo"
[[ -f "$PACKAGE_INFO" ]] || {
  echo "Expanded package is missing PackageInfo." >&2
  exit 1
}
PKG_IDENTIFIER="$(/usr/bin/xmllint --xpath 'string(/pkg-info/@identifier)' "$PACKAGE_INFO")"
PKG_VERSION="$(/usr/bin/xmllint --xpath 'string(/pkg-info/@version)' "$PACKAGE_INFO")"
INSTALL_LOCATION="$(/usr/bin/xmllint --xpath 'string(/pkg-info/@install-location)' "$PACKAGE_INFO")"
[[ "$PKG_IDENTIFIER" == "$BUNDLE_ID" ]] || {
  echo "Package identifier mismatch: ${PKG_IDENTIFIER:-<missing>}" >&2
  exit 1
}
[[ "$INSTALL_LOCATION" == "/Applications" ]] || {
  echo "Package install location mismatch: ${INSTALL_LOCATION:-<missing>}" >&2
  exit 1
}
if [[ -n "$EXPECTED_VERSION" && "$PKG_VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "Package version mismatch: ${PKG_VERSION:-<missing>} (expected ${EXPECTED_VERSION})." >&2
  exit 1
fi

APP_PATH="$TMP_DIR/expanded/Payload/${PRODUCT_NAME}.app"
if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  echo "Package payload is missing ${PRODUCT_NAME}.app." >&2
  exit 1
fi
if [[ "$ALLOW_ADHOC" == "1" ]]; then
  EXPECTED_VERSION="$EXPECTED_VERSION" \
    "${SCRIPT_DIR}/verify-app-identity.sh" --allow-adhoc "$APP_PATH"
else
  EXPECTED_VERSION="$EXPECTED_VERSION" "${SCRIPT_DIR}/verify-app-identity.sh" "$APP_PATH"
fi
grep -Eq '^\./UltraTerm\.app/Contents/MacOS/ultraterm$' "$PAYLOAD_LIST"
grep -Eq '^\./UltraTerm\.app/Contents/Resources/omp-safe$' "$PAYLOAD_LIST"
grep -Eq '^\./UltraTerm\.app/Contents/Resources/utp$' "$PAYLOAD_LIST"
grep -Eq '^\./UltraTerm\.app/Contents/Resources/omp-profile-management/SKILL\.md$' "$PAYLOAD_LIST"

if [[ "${REQUIRE_SIGNED:-0}" == "1" ]]; then
  PKG_SIGNATURE="$(pkgutil --check-signature "$PKG_PATH" 2>&1)" || {
    echo "$PKG_SIGNATURE" >&2
    exit 1
  }
  if ! grep -Eq 'Developer ID Installer: .*\(T63VT9UAY2\)' <<<"$PKG_SIGNATURE"; then
    echo "Package is not signed by the expected Developer ID Installer identity." >&2
    exit 1
  fi
  spctl --assess --type install --verbose=2 "$PKG_PATH"
fi
if [[ "${REQUIRE_NOTARIZED:-0}" == "1" ]]; then
  xcrun stapler validate "$PKG_PATH"
fi

if [[ "$INSTALL_MODE" == "--install" ]]; then
  sudo installer -pkg "$PKG_PATH" -target /
  INSTALLED_APP="/Applications/${PRODUCT_NAME}.app"
elif [[ "$INSTALL_MODE" == "--install-user" ]]; then
  echo "Per-user installation is disabled; UltraTerm must be installed in /Applications." >&2
  exit 2
else
  INSTALLED_APP=""
fi

if [[ -n "$INSTALLED_APP" ]]; then
  test -x "${INSTALLED_APP}/Contents/MacOS/ultraterm"
  if [[ "$ALLOW_ADHOC" == "1" ]]; then
    EXPECTED_VERSION="$EXPECTED_VERSION" \
      "${SCRIPT_DIR}/verify-app-identity.sh" --allow-adhoc "$INSTALLED_APP"
  else
    EXPECTED_VERSION="$EXPECTED_VERSION" \
      "${SCRIPT_DIR}/verify-app-identity.sh" "$INSTALLED_APP"
  fi
fi

echo "Verified ${PKG_PATH}"
