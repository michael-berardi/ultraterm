#!/usr/bin/env bash
set -euo pipefail

PRODUCT_NAME="UltraTerm"
BUNDLE_ID="com.libertydesignstudio.ultraterm"
TEAM_ID="T63VT9UAY2"
EXPECTED_VERSION="${EXPECTED_VERSION:-}"
ALLOW_ADHOC=0

usage() {
  echo "Usage: $0 [--allow-adhoc] /path/to/UltraTerm.app" >&2
}

if [[ "${1:-}" == "--allow-adhoc" ]]; then
  ALLOW_ADHOC=1
  shift
fi
APP_PATH="${1:-}"
if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  usage
  exit 1
fi

if [[ "$(basename "$APP_PATH")" != "${PRODUCT_NAME}.app" ]]; then
  echo "Expected ${PRODUCT_NAME}.app, got: $APP_PATH" >&2
  exit 1
fi

if ! /usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH" >/dev/null 2>&1; then
  echo "${PRODUCT_NAME}.app has invalid or unsealed code resources: $APP_PATH" >&2
  exit 1
fi

DETAILS="$(/usr/bin/codesign -dv --verbose=4 "$APP_PATH" 2>&1)" || {
  echo "Unable to inspect the code signature: $APP_PATH" >&2
  exit 1
}
if ! /usr/bin/grep -Fqx -- "Identifier=${BUNDLE_ID}" <<<"$DETAILS"; then
  echo "Unexpected bundle identifier in $APP_PATH" >&2
  exit 1
fi
if [[ -n "$EXPECTED_VERSION" ]]; then
  VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
    "$APP_PATH/Contents/Info.plist" 2>/dev/null || true)"
  if [[ "$VERSION" != "$EXPECTED_VERSION" ]]; then
    echo "Unexpected app version ${VERSION:-<missing>} (expected ${EXPECTED_VERSION}): $APP_PATH" >&2
    exit 1
  fi
fi

if [[ "$ALLOW_ADHOC" != "1" ]]; then
  if ! /usr/bin/grep -Fqx -- "TeamIdentifier=${TEAM_ID}" <<<"$DETAILS"; then
    echo "Unexpected Developer Team in $APP_PATH" >&2
    exit 1
  fi
  if ! /usr/bin/grep -Eq '^Authority=Developer ID Application: .+ \('"$TEAM_ID"'\)$' <<<"$DETAILS"; then
    echo "${PRODUCT_NAME}.app is not signed by the expected Developer ID Application identity: $APP_PATH" >&2
    exit 1
  fi
  if ! /usr/bin/grep -Ei 'flags=.*runtime' <<<"$DETAILS" >/dev/null; then
    echo "${PRODUCT_NAME}.app is missing the hardened runtime: $APP_PATH" >&2
    exit 1
  fi

  REQUIREMENTS="$(/usr/bin/codesign -d -r- "$APP_PATH" 2>&1)" || {
    echo "Unable to inspect the designated requirement: $APP_PATH" >&2
    exit 1
  }
  if ! /usr/bin/grep -Fq -- 'designated =>' <<<"$REQUIREMENTS" || \
     ! /usr/bin/grep -Fq -- "identifier \"${BUNDLE_ID}\"" <<<"$REQUIREMENTS" || \
     ! /usr/bin/grep -Fq -- 'anchor apple generic' <<<"$REQUIREMENTS" || \
     ! /usr/bin/grep -Eq -- "certificate .*OU.*${TEAM_ID}" <<<"$REQUIREMENTS"; then
    echo "${PRODUCT_NAME}.app has an unexpected designated requirement: $APP_PATH" >&2
    exit 1
  fi
fi

echo "Verified ${PRODUCT_NAME}.app identity: $APP_PATH"
