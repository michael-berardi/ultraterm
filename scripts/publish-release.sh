#!/usr/bin/env bash
set -euo pipefail

export PATH="/opt/homebrew/bin:/usr/local/bin:${PATH}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-$(node -p "require('${ROOT_DIR}/package.json').version")}"
TAG="v${VERSION}"
OUTPUT_DIR="${OUTPUT_DIR:-${ROOT_DIR}/dist/release}"

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

if [[ "${SKIP_PACKAGE:-0}" != "1" ]]; then
  OUTPUT_DIR="$OUTPUT_DIR" "${ROOT_DIR}/scripts/package-release.sh"
fi
shopt -s nullglob
ASSETS=("${OUTPUT_DIR}"/*.pkg "${OUTPUT_DIR}"/*.pkg.sha256 "${OUTPUT_DIR}"/*.zip "${OUTPUT_DIR}"/*.zip.sha256)
if [[ ${#ASSETS[@]} -ne 4 ]]; then
  echo "Expected four release assets in $OUTPUT_DIR." >&2
  exit 1
fi

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
