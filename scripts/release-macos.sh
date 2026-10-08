#!/usr/bin/env bash
set -euo pipefail

# Signing credentials stay in the environment; never expand them into commands.
set +x
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
  if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:+set}" || -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+set}" ]]; then
    echo "Release refused: set TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD (an empty password is allowed). Signing credentials must be supplied via environment variables." >&2
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
const needsLatest = (config.plugins?.updater?.endpoints || []).some(url => url.includes('latest.json'));
console.log(version);
console.log(name);
console.log(needsLatest);
console.log(path.resolve(process.env.CARGO_TARGET_DIR || 'src-tauri/target'));
JS
)"
IFS=$'\n' read -r -d '' VERSION PRODUCT_NAME NEEDS_LATEST CARGO_TARGET_DIR <<< "$METADATA" || true
export CARGO_TARGET_DIR
TAG="md-sticky-v$VERSION"
OUTPUT_DIR="$ROOT_DIR/dist/$TAG"
BUILD_CONFIG='{"bundle":{"targets":["app","dmg"],"createUpdaterArtifacts":true}}'
ARTIFACTS=()

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
  ARCHIVE="$BUNDLE_DIR/macos/$PRODUCT_NAME.app.tar.gz"
  # Both target builds use the same archive basename; give release assets unique names.
  RELEASE_ARCHIVE="$OUTPUT_DIR/${PRODUCT_NAME}_${VERSION}_${ARCH}.app.tar.gz"
  for artifact in "$DMG" "$ARCHIVE" "$ARCHIVE.sig"; do
    require_artifact "$artifact"
  done
  run cp "$DMG" "$OUTPUT_DIR/"
  run cp "$ARCHIVE" "$RELEASE_ARCHIVE"
  run cp "$ARCHIVE.sig" "$RELEASE_ARCHIVE.sig"
  ARTIFACTS+=("$OUTPUT_DIR/$(basename "$DMG")" "$RELEASE_ARCHIVE" "$RELEASE_ARCHIVE.sig")
done

if [[ "$NEEDS_LATEST" == true ]]; then
  # A static updater endpoint needs a manifest matching the renamed signed assets.
  LATEST_CODE='const fs = require("node:fs");
const [version, name, dir, tag, repo] = process.argv.slice(1);
const platforms = {};
for (const [arch, key] of [["aarch64", "darwin-aarch64"], ["x86_64", "darwin-x86_64"]]) {
  const file = `${name}_${version}_${arch}.app.tar.gz`;
  platforms[key] = {
    signature: fs.readFileSync(`${dir}/${file}.sig`, "utf8").trim(),
    url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(file)}`
  };
}
fs.writeFileSync(`${dir}/latest.json`, JSON.stringify({version, notes: "See the assets to download this version and install.", pub_date: new Date().toISOString(), platforms}, null, 2) + "\n");'
  if "$DRY_RUN"; then
    echo 'RELEASE_REPO=$(gh repo view --json nameWithOwner --jq .nameWithOwner)'
    printf '%q ' node --input-type=commonjs -e "$LATEST_CODE" "$VERSION" "$PRODUCT_NAME" "$OUTPUT_DIR" "$TAG"
    printf '"$RELEASE_REPO"\n'
  else
    RELEASE_REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
    run node --input-type=commonjs -e "$LATEST_CODE" "$VERSION" "$PRODUCT_NAME" "$OUTPUT_DIR" "$TAG" "$RELEASE_REPO"
  fi
  ARTIFACTS+=("$OUTPUT_DIR/latest.json")
fi

run gh release create "$TAG" --draft --target "$(git rev-parse HEAD)" \
  --title "Md-Sticky v$VERSION" \
  --notes 'See the assets to download this version and install.' \
  "${ARTIFACTS[@]}"
