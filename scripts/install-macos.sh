#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_APP="$ROOT_DIR/src-tauri/target/release/bundle/macos/Sticky.app"
APPLICATIONS_DIR="/Applications"
TARGET_APP="$APPLICATIONS_DIR/Sticky.app"
STAGING_APP="$APPLICATIONS_DIR/.Sticky.install.$$"
BACKUP_APP="$APPLICATIONS_DIR/.Sticky.backup.$$"
APP_BUNDLE_ID="local.jian.mdsticky"
APP_PROCESS="md-sticky-local"
APP_DATA_DIR="$HOME/Library/Application Support/$APP_BUNDLE_ID"
NOTES_FILE="$APP_DATA_DIR/notes.json"
PREINSTALL_NOTES="$APP_DATA_DIR/notes.preinstall.json"
PREINSTALL_STAGING="$APP_DATA_DIR/.notes.preinstall.$$"

cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This installer only supports macOS." >&2
  exit 1
fi

for command in cargo codesign ditto npm open osascript pgrep; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing required command: $command" >&2
    exit 1
  fi
done

if [[ ! -w "$APPLICATIONS_DIR" ]]; then
  echo "$APPLICATIONS_DIR is not writable by this account." >&2
  echo "Give your account permission to install apps there, then run npm run install:macos again." >&2
  exit 1
fi

rollback() {
  rm -rf "$STAGING_APP"
  rm -f "$PREINSTALL_STAGING"
  if [[ ! -e "$TARGET_APP" && -e "$BACKUP_APP" ]]; then
    mv "$BACKUP_APP" "$TARGET_APP"
  fi
}
trap rollback EXIT

if [[ ! -d node_modules ]]; then
  echo "Frontend dependencies are missing. Run: ./install.sh" >&2
  exit 1
fi

echo "Building Sticky..."
npm run app:build

if [[ ! -d "$SOURCE_APP" ]]; then
  echo "Build completed without producing $SOURCE_APP" >&2
  exit 1
fi

echo "Preparing the new app..."
rm -rf "$STAGING_APP" "$BACKUP_APP"
ditto "$SOURCE_APP" "$STAGING_APP"
codesign --verify --deep --strict "$STAGING_APP"

if [[ -f "$NOTES_FILE" ]]; then
  echo "Preserving a pre-install note snapshot..."
  mkdir -p "$APP_DATA_DIR"
  ditto "$NOTES_FILE" "$PREINSTALL_STAGING"
  mv -f "$PREINSTALL_STAGING" "$PREINSTALL_NOTES"
fi

echo "Saving notes and quitting the running app..."
if pgrep -x "$APP_PROCESS" >/dev/null 2>&1; then
  osascript -e "tell application id \"$APP_BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
fi

for _ in {1..100}; do
  if ! pgrep -x "$APP_PROCESS" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

if pgrep -x "$APP_PROCESS" >/dev/null 2>&1; then
  echo "Sticky did not finish quitting, so the installed app was left unchanged." >&2
  exit 1
fi

echo "Installing Sticky in Applications..."
if [[ -e "$TARGET_APP" ]]; then
  mv "$TARGET_APP" "$BACKUP_APP"
fi
mv "$STAGING_APP" "$TARGET_APP"

if ! codesign --verify --deep --strict "$TARGET_APP"; then
  rm -rf "$TARGET_APP"
  if [[ -e "$BACKUP_APP" ]]; then
    mv "$BACKUP_APP" "$TARGET_APP"
  fi
  echo "The new app failed verification. The previous installation was restored." >&2
  exit 1
fi

rm -rf "$BACKUP_APP"
trap - EXIT

echo "Opening the installed app..."
open "$TARGET_APP"

echo
echo "Done: $TARGET_APP"
echo "Your notes remain in ~/Library/Application Support/$APP_BUNDLE_ID/notes.json"
