# StickyMD

A local Markdown sticky-note app for Apple Silicon Macs. The installed app is named **Sticky**.

https://github.com/user-attachments/assets/7b61d4fa-8e2a-4b80-af09-37120dd7e8cb

## Install

Open **Terminal**, paste this entire command, and press **Return**:

```sh
/bin/bash -c "$(/usr/bin/curl -fsSL https://raw.githubusercontent.com/jkuang7/StickyMD/main/scripts/bootstrap-macos.sh)"
```

That command handles everything and opens Sticky when it is done. The first installation can take several minutes. If your Mac asks to install developer tools, click **Install**, wait for it to finish, then return to Terminal and press **Return**.

## Update

1. Open **Sticky**.
2. In the menu bar at the top of the screen, click **Help**, then **Update…**.
3. If you see **Update available**, click **Update**.
4. Leave the Terminal window open while Sticky updates. Sticky will close and reopen when it is done.

If you see **Up to date**, you already have the latest version. Your notes stay safe during an update.

## If macOS blocks Sticky

Sticky is built on your Mac and is not notarized by Apple. Try to open Sticky once, then go to **System Settings → Privacy & Security**, scroll to **Security**, and click **Open Anyway**.

## Your notes

Sticky has no account, analytics, or cloud sync. Notes stay on your Mac in:

```text
~/Library/Application Support/local.jian.mdsticky/
```

Press `Command-/` inside Sticky to see its keyboard shortcuts.

## Uninstall

Quit Sticky, move `/Applications/Sticky.app` to the Trash, and delete `~/StickyMD`. Your saved notes remain in the folder above unless you delete it too.

Development and architecture details are in [PLOT.md](PLOT.md).
