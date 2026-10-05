# Restore All review evidence

Assignment: [StickyMD #16](https://github.com/jkuang7/StickyMD/issues/16).
Incoming candidate: [557b64d](https://github.com/jkuang7/StickyMD/commit/557b64d39f9087c62aaef6ddb7bb8b25ecf05a27).

## Incoming candidate native check, 2026-10-05

Built the exact incoming source using the configured `app:build` command with
`--debug` and an isolated identifier/product name, `local.jian.mdsticky.review16`
and `Sticky Review16`. The user's installed Sticky and its data were untouched.

The disposable version-4 repository represented a supported archived state:
one active group anchor, two archived members of that group, and one standalone
archived note, with distinct recent archive timestamps and valid documents.
Selected Restore All Notes in the native menu. Each of the three restored notes
was inspected through native accessibility while focusing and archiving it.
All archive timestamps cleared; saved group positions were (600,100),
(600,362), and (600,624), matching 250-point heights and 12-point gaps.

Archived the three notes again through Cmd+W, saved the resulting store bytes,
and made only the disposable app-data directory read-only (mode 0500).
Selected Restore All Notes again. The store bytes, original archive timestamps,
and saved positions remained unchanged. Focus Next Note returned to the sole
remaining anchor, and native destruction logs recorded all three staged
archived windows closing. No in-app error dialog appeared. The only action
failure was logged by `handle_menu_event`:

    Error executing command: RestoreAllNotes : Failed to write temporary storage: Permission denied (os error 13); rollback failed: Failed to write temporary storage: Permission denied (os error 13)

This proves the missing user-visible failure required by acceptance 2 and the
child's real-world check. Repository atomicity and window cleanup passed.

## Correction and verification

Restore All's existing public wrapper now sends its complete failure through
the existing native action failure renderer exactly once, preserving the error
for logging. This includes precommit, rollback, and postcommit failures without
changing the transaction boundary or Restore Last.

Restore All now uses the existing `persist_surface_changes` helper for original
surface capture, durable batch persistence, and persistence-failure rollback.
Its staged originals remain available if the later archive commit fails.

The rollback tests retain checks for complete geometry/durable-state restoration,
closing every staged window, attempting every recovery step, and including each
failure once. They no longer duplicate an identical stage or require a particular
rollback iteration order or formatted error sentence.

Focused command:

    CARGO_TARGET_DIR=/tmp/stickymd-sandcastle-target cargo test --manifest-path src-tauri/Cargo.toml restore_all_ -- --nocapture

Result: all five tests passed, including failed archive commit preservation on
disk/in memory, successful restoration, no-op persistence, retention, partial
window creation, and geometry/window rollback. The controller's passing
incoming typecheck and full-suite results were not rerun.

## Native verification limits

Computer-use Cmd+Shift+U input did not dispatch the menu callback, so the native
menu item wired to the same command was used. The physical shortcut remains
unverified by this run.

A corrected build exercised the same read-only failure and kept the store
unchanged. The native dialog API hosts its unparented macOS alert through
UserNotificationCenter. Computer use refused access to
`com.apple.UserNotificationCenter` for safety reasons, so the corrected alert's
appearance could not be inspected. The final wrapper reuses the existing
`AppNativeUserActionTransport::render` failure path; source inspection establishes
the single call and complete error payload. Fresh review must complete the
visual alert and physical-shortcut check.

The parent check is deferred to fresh review because the incoming child needed
correction. Disposable app data, logs, fixtures, and application were removed;
this evidence is retained.
