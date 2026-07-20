#!/usr/bin/env bash

set -euo pipefail

printf '\nUpdating Sticky...\n'
printf 'No input is needed in Terminal. Keep this window open; Sticky will reopen when the update finishes.\n\n'

exec "$HOME/StickyMD/scripts/bootstrap-macos.sh"
