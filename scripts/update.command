#!/usr/bin/env bash

set -euo pipefail

# Terminal refuses to close a window whose shell is still working, so the
# window this update runs in has to be closed after the script exits. Identify
# it now: among the tabs reporting our terminal device, ours is the busy one.
# A finished window keeps reporting a device that a later window can reuse, so
# matching on the device alone would risk closing somebody else's window.
terminal_window_id() {
  local tty_path
  tty_path="$(/usr/bin/tty 2>/dev/null || true)"
  [[ -n "$tty_path" ]] || return 0

  /usr/bin/osascript - "$tty_path" 2>/dev/null <<'APPLESCRIPT' || true
on run argv
  set targetTTY to item 1 of argv
  tell application "Terminal"
    repeat with theWindow in windows
      repeat with theTab in tabs of theWindow
        if tty of theTab is targetTTY and busy of theTab then
          return (id of theWindow) as string
        end if
      end repeat
    end repeat
  end tell
  return ""
end run
APPLESCRIPT
}

# Waits for this shell to finish, then closes the window it ran in. Detached so
# that it outlives the script; best effort, because a window that cannot be
# scripted simply stays open the way it always did.
close_terminal_window_after_exit() {
  local window_id="$1"

  nohup /usr/bin/osascript - "$window_id" </dev/null >/dev/null 2>&1 <<'APPLESCRIPT' &
on run argv
  set targetID to (item 1 of argv) as integer
  tell application "Terminal"
    repeat 80 times
      try
        set theWindow to first window whose id is targetID
        if not (busy of (first tab of theWindow)) then
          close theWindow saving no
          return
        end if
      on error
        return
      end try
      delay 0.25
    end repeat
  end tell
end run
APPLESCRIPT
  disown 2>/dev/null || true
}

window_id="$(terminal_window_id)"

printf '\nUpdating Sticky...\n'
printf 'No input is needed while this runs. Sticky will reopen when the update finishes.\n\n'

status=0
"$HOME/StickyMD/scripts/bootstrap-macos.sh" || status=$?

if (( status == 0 )); then
  printf '\nSticky is up to date.\n'
else
  printf '\nThe update did not finish. The messages above explain why.\n'
fi

if [[ -t 0 ]]; then
  printf '\nPress any key to close this window... '
  read -r -n 1 -s || true
  printf '\n'
fi

if [[ -n "$window_id" ]]; then
  close_terminal_window_after_exit "$window_id"
fi

exit "$status"
