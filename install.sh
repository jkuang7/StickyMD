#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TOOLS_DIR="$ROOT_DIR/.tools"
NODE_DIR="$TOOLS_DIR/node"
NODE_LTS_MAJOR="24"
export CARGO_HOME="$TOOLS_DIR/cargo"
export RUSTUP_HOME="$TOOLS_DIR/rustup"
FRONTEND_DEPENDENCY_STATE="$TOOLS_DIR/frontend-dependencies.sha256"
APPLE_TOOLS_WAIT_SECONDS=3600

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sticky-install.XXXXXX")"
cleanup() {
  rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

step() {
  printf '\n==> %s\n' "$1"
}

fail() {
  printf '\nSticky was not installed: %s\n' "$1" >&2
  printf 'Fix the problem above, then run ./install.sh again.\n' >&2
  exit 1
}

wait_for_apple_tools() {
  local elapsed=0

  while ! /usr/bin/xcrun --find clang >/dev/null 2>&1; do
    if (( elapsed >= APPLE_TOOLS_WAIT_SECONDS )); then
      fail "Apple's Command Line Tools did not finish installing."
    fi
    /bin/sleep 5
    elapsed=$((elapsed + 5))
  done
}

ensure_apple_tools() {
  if /usr/bin/xcrun --find clang >/dev/null 2>&1; then
    return
  fi

  printf 'Apple needs to install its free Command Line Tools before Sticky can be built.\n'
  printf 'A macOS installation window should appear now.\n'
  /usr/bin/xcode-select --install >/dev/null 2>&1 || true
  printf 'Click Install in that window. Sticky will continue automatically when it finishes.\n'
  wait_for_apple_tools
}

install_node() {
  local base_url checksums archive expected actual extracted

  if [[ -x "$NODE_DIR/bin/node" && -x "$NODE_DIR/bin/npm" ]]; then
    return
  fi

  base_url="https://nodejs.org/dist/latest-v${NODE_LTS_MAJOR}.x"
  checksums="$(/usr/bin/curl --fail --silent --show-error --location --retry 3 \
    "$base_url/SHASUMS256.txt")" || fail "Node.js could not be downloaded. Check your internet connection."
  archive="$(printf '%s\n' "$checksums" | /usr/bin/awk \
    '$2 ~ /^node-v[0-9.]+-darwin-arm64\.tar\.gz$/ { print $2; exit }')"
  [[ -n "$archive" ]] || fail "Node.js did not publish the expected Apple Silicon download."

  printf 'Downloading %s...\n' "$archive"
  /usr/bin/curl --fail --show-error --location --retry 3 --progress-bar \
    "$base_url/$archive" --output "$TEMP_DIR/$archive" || \
    fail "Node.js could not be downloaded. Check your internet connection."

  expected="$(printf '%s\n' "$checksums" | /usr/bin/awk -v file="$archive" \
    '$2 == file { print $1; exit }')"
  actual="$(/usr/bin/shasum -a 256 "$TEMP_DIR/$archive" | /usr/bin/awk '{ print $1 }')"
  [[ -n "$expected" && "$actual" == "$expected" ]] || \
    fail "The Node.js download failed its security checksum."

  extracted="$TEMP_DIR/node"
  /bin/mkdir -p "$extracted"
  /usr/bin/tar -xzf "$TEMP_DIR/$archive" -C "$extracted" --strip-components=1
  [[ -x "$extracted/bin/node" && -x "$extracted/bin/npm" ]] || \
    fail "The Node.js download did not contain the expected tools."

  /bin/mkdir -p "$TOOLS_DIR"
  /bin/rm -rf "$NODE_DIR"
  /bin/mv "$extracted" "$NODE_DIR"
}

install_rust() {
  local rustup_script

  if [[ ! -x "$CARGO_HOME/bin/rustup" ]]; then
    rustup_script="$TEMP_DIR/rustup-init.sh"
    printf 'Downloading the official Rust installer...\n'
    /usr/bin/curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
      --location --retry 3 https://sh.rustup.rs --output "$rustup_script" || \
      fail "Rust could not be downloaded. Check your internet connection."
    /bin/sh "$rustup_script" -y --no-modify-path --profile minimal \
      --default-toolchain stable || fail "Rust could not be installed."
  fi

  export PATH="$CARGO_HOME/bin:$NODE_DIR/bin:$PATH"
  if ! "$CARGO_HOME/bin/rustfmt" --version >/dev/null 2>&1; then
    "$CARGO_HOME/bin/rustup" component add rustfmt || \
      fail "Rust's formatting tool could not be installed."
  fi
}

frontend_dependency_fingerprint() {
  {
    /usr/bin/shasum -a 256 "$ROOT_DIR/package.json" "$ROOT_DIR/package-lock.json" | \
      /usr/bin/awk '{ print $1 }'
    node --version
    npm --version
  } | /usr/bin/shasum -a 256 | /usr/bin/awk '{ print $1 }'
}

install_frontend_dependencies() {
  local current_fingerprint installed_fingerprint=""

  current_fingerprint="$(frontend_dependency_fingerprint)"
  if [[ -f "$FRONTEND_DEPENDENCY_STATE" ]]; then
    installed_fingerprint="$(<"$FRONTEND_DEPENDENCY_STATE")"
  fi

  if [[ -d "$ROOT_DIR/node_modules" && \
        "$installed_fingerprint" == "$current_fingerprint" ]]; then
    printf 'Ready: project dependencies are unchanged.\n'
    return
  fi

  /bin/rm -f "$FRONTEND_DEPENDENCY_STATE"
  npm ci
  printf '%s\n' "$current_fingerprint" > "$TEMP_DIR/frontend-dependencies.sha256"
  /bin/mkdir -p "$TOOLS_DIR"
  /bin/mv "$TEMP_DIR/frontend-dependencies.sha256" "$FRONTEND_DEPENDENCY_STATE"
}

step "Checking this Mac"
[[ "$(/usr/bin/uname -s)" == "Darwin" ]] || fail "This installer only supports macOS."
[[ "$(/usr/bin/uname -m)" == "arm64" ]] || fail "This installer only supports Apple Silicon Macs."
ensure_apple_tools
command -v git >/dev/null 2>&1 || fail "Git is missing. Run xcode-select --install and try again."
printf 'Ready: %s\n' "$(git --version)"

step "Preparing Node.js"
install_node
export PATH="$NODE_DIR/bin:$CARGO_HOME/bin:$PATH"
printf 'Ready: Node.js %s, npm %s\n' "$(node --version)" "$(npm --version)"

step "Preparing Rust"
install_rust
printf 'Ready: %s\n' "$(rustc --version)"

step "Preparing Sticky's project dependencies"
cd "$ROOT_DIR"
install_frontend_dependencies

step "Building and installing Sticky"
if ! npm run install:macos; then
  /bin/rm -f "$FRONTEND_DEPENDENCY_STATE"
  exit 1
fi
