#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

DRY_RUN=false
case "${1:-}" in
  --dry-run) DRY_RUN=true ;;
  '') ;;
  *) echo "Usage: npm run release:macos -- [--dry-run]" >&2; exit 1 ;;
esac
if [[ $# -gt 1 ]]; then
  echo "Usage: npm run release:macos -- [--dry-run]" >&2
  exit 1
fi

if ! "$DRY_RUN"; then
  TREE_STATUS="$(git --no-optional-locks status --porcelain)"
  if [[ -n "$TREE_STATUS" ]]; then
    echo "Release refused: working tree is dirty. Commit or remove changes before releasing." >&2
    exit 1
  fi
fi

# Resolve Tauri's version-file form and refuse inconsistent release versions.
METADATA="$(node --input-type=commonjs <<'JS'
const fs = require('node:fs');
const path = require('node:path');
const pkg = JSON.parse(fs.readFileSync('package.json', 'utf8'));
const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
let version = config.version || pkg.version;
if (version.endsWith('.json')) {
  version = JSON.parse(fs.readFileSync(path.resolve('src-tauri', version), 'utf8')).version;
}
if (version !== pkg.version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error('package.json and tauri.conf.json must specify the same valid release version');
}
const name = config.productName || pkg.name;
if (!/^[A-Za-z0-9 ._-]+$/.test(name)) throw new Error('Unsupported productName for macOS artifact paths');
console.log(version);
console.log(name);
console.log(path.resolve(process.env.CARGO_TARGET_DIR || 'src-tauri/target'));
JS
)"
IFS=$'\n' read -r -d '' VERSION PRODUCT_NAME CARGO_TARGET_DIR <<< "$METADATA" || true
export CARGO_TARGET_DIR
TAG="md-sticky-v$VERSION"
OUTPUT_DIR="$ROOT_DIR/dist/$TAG"
BUILD_CONFIG='{"bundle":{"targets":["app","dmg"]}}'
ARTIFACTS=()

if "$DRY_RUN"; then
  echo '# Dry run: skipping clean-tree, pushed-HEAD, existing-tag, and build prerequisites.'
  echo 'RELEASE_SHA=$(git rev-parse HEAD)'
  RELEASE_SHA='$RELEASE_SHA'
else
  RELEASE_SHA="$(git rev-parse HEAD)"
  if [[ -z "$(git branch -r --contains "$RELEASE_SHA")" ]]; then
    echo "Release refused: HEAD is not contained in a remote branch. Push it before releasing." >&2
    exit 1
  fi
  if git show-ref --verify --quiet "refs/tags/$TAG"; then
    echo "Release refused: tag $TAG already exists locally." >&2
    exit 1
  fi
  if ! REMOTE_TAG="$(git ls-remote --tags origin "refs/tags/$TAG" "refs/tags/$TAG^{}")"; then
    echo "Release refused: could not check origin for tag $TAG." >&2
    exit 1
  fi
  if [[ -n "$REMOTE_TAG" ]]; then
    echo "Release refused: tag $TAG already exists on origin." >&2
    exit 1
  fi
  if [[ "$(uname -s)" != Darwin ]]; then
    echo "Release requires macOS." >&2
    exit 1
  fi
  for command in node npm cargo rustup gh; do
    command -v "$command" >/dev/null || { echo "Missing required command: $command" >&2; exit 1; }
  done
  [[ -d node_modules ]] || { echo "Frontend dependencies missing: run npm ci." >&2; exit 1; }
  installed_targets="$(rustup target list --installed)"
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    [[ "$installed_targets" == *"$target"* ]] || { echo "Missing Rust target: run rustup target add $target" >&2; exit 1; }
  done
fi

run() {
  if "$DRY_RUN"; then
    printf '%q ' "$@"
    printf '\n'
  else
    "$@"
  fi
}

require_artifact() {
  if ! "$DRY_RUN" && [[ ! -s "$1" ]]; then
    echo "Release refused: missing or empty artifact: $1" >&2
    exit 1
  fi
}

if "$DRY_RUN"; then
  printf 'cd %q\n' "$ROOT_DIR"
  printf 'export CARGO_TARGET_DIR=%q\n' "$CARGO_TARGET_DIR"
fi
run gh auth status
run mkdir -p "$OUTPUT_DIR"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  run npm run tauri -- build --target "$target" --config "$BUILD_CONFIG"
  BUNDLE_DIR="$CARGO_TARGET_DIR/$target/release/bundle"
  ARCH="${target%%-*}"
  DMG_ARCH="$ARCH"
  [[ "$ARCH" != x86_64 ]] || DMG_ARCH=x64
  DMG="$BUNDLE_DIR/dmg/${PRODUCT_NAME}_${VERSION}_${DMG_ARCH}.dmg"
  require_artifact "$DMG"
  run cp "$DMG" "$OUTPUT_DIR/"
  ARTIFACTS+=("$OUTPUT_DIR/$(basename "$DMG")")
done

run gh release create "$TAG" --draft --target "$RELEASE_SHA" \
  --title "Md-Sticky v$VERSION" \
  --notes 'See the assets to download this version and install.' \
  "${ARTIFACTS[@]}"
