#!/bin/bash
set -euo pipefail

PRODUCT_NAME="UltraTerm"
BUNDLE_ID="com.libertydesignstudio.ultraterm"
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
grep -Eq "(^|/)${PRODUCT_NAME}\\.app/Contents/MacOS/" "$PAYLOAD_LIST"

if [[ "${REQUIRE_SIGNED:-0}" == "1" ]]; then
  pkgutil --check-signature "$PKG_PATH"
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
  INSTALLED_ID="$(codesign -dv "$INSTALLED_APP" 2>&1 | sed -n 's/^Identifier=//p')"
  test "$INSTALLED_ID" = "$BUNDLE_ID"
  codesign --verify --deep --strict --verbose=2 "$INSTALLED_APP"
fi

echo "Verified ${PKG_PATH}"
