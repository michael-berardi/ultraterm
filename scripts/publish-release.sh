#!/usr/bin/env bash
set -euo pipefail

export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:${PATH}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-$(node -p "require('${ROOT_DIR}/package.json').version")}"
TAG="v${VERSION}"
OUTPUT_DIR="${OUTPUT_DIR:-${ROOT_DIR}/release}"

if ! command -v gh >/dev/null 2>&1; then
  echo "GitHub CLI is required: https://cli.github.com/" >&2
  exit 1
fi
if ! gh auth status >/dev/null 2>&1; then
  echo "Authenticate GitHub CLI with: gh auth login" >&2
  exit 1
fi
if [[ -n "$(git -C "$ROOT_DIR" status --porcelain --untracked-files=no)" ]]; then
  echo "Commit tracked changes before publishing a release." >&2
  exit 1
fi

if [[ "${ALLOW_ADHOC:-0}" == "1" ]]; then
  echo "Public releases cannot use ad-hoc signatures." >&2
  exit 1
fi
if [[ "${SKIP_PACKAGE:-0}" != "1" ]]; then
  OUTPUT_DIR="$OUTPUT_DIR" "${ROOT_DIR}/scripts/package-release.sh"
fi

PKG_PATH="${OUTPUT_DIR}/UltraTerm-macos-arm64.pkg"
PKG_CHECKSUM="${PKG_PATH}.sha256"
ARCHIVE_PATH="${OUTPUT_DIR}/UltraTerm-macos-arm64.zip"
ARCHIVE_CHECKSUM="${ARCHIVE_PATH}.sha256"
ASSETS=("$PKG_PATH" "$PKG_CHECKSUM" "$ARCHIVE_PATH" "$ARCHIVE_CHECKSUM")
for asset in "${ASSETS[@]}"; do
  [[ -f "$asset" ]] || {
    echo "Missing required release asset: $asset" >&2
    exit 1
  }
done
(
  cd "$OUTPUT_DIR"
  shasum -a 256 --check "$(basename "$PKG_CHECKSUM")"
  shasum -a 256 --check "$(basename "$ARCHIVE_CHECKSUM")"
)
EXPECTED_VERSION="$VERSION" REQUIRE_SIGNED=1 REQUIRE_NOTARIZED=1 ALLOW_ADHOC=0 \
  "${ROOT_DIR}/scripts/verify-pkg.sh" "$PKG_PATH"

ARCHIVE_CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ultraterm-publish-check.XXXXXX")"
trap 'rm -rf "$ARCHIVE_CHECK_DIR"' EXIT
ditto -x -k "$ARCHIVE_PATH" "$ARCHIVE_CHECK_DIR"
ARCHIVE_APP="${ARCHIVE_CHECK_DIR}/UltraTerm-macos-arm64/UltraTerm.app"
EXPECTED_VERSION="$VERSION" "${ROOT_DIR}/scripts/verify-app-identity.sh" "$ARCHIVE_APP"
spctl --assess --type execute --verbose=2 "$ARCHIVE_APP"
xcrun stapler validate "$ARCHIVE_APP"

HEAD_SHA="$(git -C "$ROOT_DIR" rev-parse HEAD)"
if git -C "$ROOT_DIR" rev-parse "$TAG" >/dev/null 2>&1; then
  TAG_SHA="$(git -C "$ROOT_DIR" rev-list -n 1 "$TAG")"
  if [[ "$TAG_SHA" != "$HEAD_SHA" ]]; then
    echo "$TAG already points to a different commit." >&2
    exit 1
  fi
else
  git -C "$ROOT_DIR" tag -a "$TAG" -m "UltraTerm $TAG"
fi

git -C "$ROOT_DIR" push origin HEAD
git -C "$ROOT_DIR" push origin "$TAG"
if gh release view "$TAG" --repo michael-berardi/ultraterm >/dev/null 2>&1; then
  gh release upload "$TAG" "${ASSETS[@]}" --repo michael-berardi/ultraterm --clobber
else
  gh release create "$TAG" "${ASSETS[@]}" --repo michael-berardi/ultraterm \
    --verify-tag --generate-notes --title "UltraTerm $TAG"
fi
printf 'Published https://github.com/michael-berardi/ultraterm/releases/tag/%s\n' "$TAG"
