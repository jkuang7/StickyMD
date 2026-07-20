#!/usr/bin/env bash

set -euo pipefail

REPOSITORY="https://github.com/jkuang7/StickyMD.git"
INSTALL_DIR="$HOME/StickyMD"
APPLE_TOOLS_WAIT_SECONDS=3600

fail() {
  printf '\nSticky was not installed: %s\n' "$1" >&2
  exit 1
}

wait_for_apple_tools() {
  local elapsed=0

  while ! /usr/bin/xcrun --find clang >/dev/null 2>&1; do
    if (( elapsed >= APPLE_TOOLS_WAIT_SECONDS )); then
      fail "Apple's developer tools did not finish installing. Complete their installation, then run this command again."
    fi
    /bin/sleep 5
    elapsed=$((elapsed + 5))
  done
}

[[ "$(/usr/bin/uname -s)" == "Darwin" ]] || fail "this installer only supports macOS."
[[ "$(/usr/bin/uname -m)" == "arm64" ]] || fail "this installer requires an Apple Silicon Mac."

if ! /usr/bin/xcrun --find clang >/dev/null 2>&1; then
  printf "Your Mac needs Apple's free developer tools.\n"
  /usr/bin/xcode-select --install >/dev/null 2>&1 || true
  printf 'Click Install in the window that appears. Sticky will continue automatically when it finishes.\n'
  wait_for_apple_tools
fi

command -v git >/dev/null 2>&1 || fail "Git is still unavailable. Restart Terminal and run this command again."

if [[ -e "$INSTALL_DIR" && ! -d "$INSTALL_DIR/.git" ]]; then
  fail "$INSTALL_DIR already exists but is not a StickyMD checkout. Move or rename it, then run this command again."
fi

if [[ -d "$INSTALL_DIR/.git" ]]; then
  origin="$(git -C "$INSTALL_DIR" remote get-url origin 2>/dev/null || true)"
  case "$origin" in
    "$REPOSITORY"|git@github.com:jkuang7/StickyMD.git)
      ;;
    *)
      fail "$INSTALL_DIR belongs to a different repository. Move or rename it, then run this command again."
      ;;
  esac
  printf 'Updating StickyMD...\n'
  git -C "$INSTALL_DIR" pull --ff-only || \
    fail "StickyMD could not be updated. Check the message above."
else
  printf 'Downloading StickyMD...\n'
  git clone "$REPOSITORY" "$INSTALL_DIR" || \
    fail "StickyMD could not be downloaded. Check your internet connection."
fi

exec "$INSTALL_DIR/install.sh"
