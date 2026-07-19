# Sticky Architecture

## Note ownership

- Rust is the sole owner of durable note state. `NoteRepository` serializes every read-modify-write operation behind one mutex and atomically replaces `notes.json` only after the temporary file has been flushed.
- Each note has a stable UUID and stores Tiptap JSON, color, logical position, expanded dimensions, collapsed state, and pin state. Window labels derive from the UUID and are not persistence identities themselves.
- The webview owns only the live Tiptap editor instance. It sends complete document snapshots to Rust on debounced edits and flushes them at blur, fold, close, and application-quit boundaries.
- Clipboard interoperability uses Tiptap's schema-backed defaults: copy provides HTML and plain-text representations, while paste retains supported semantic structure and discards unsupported markup.

## Snapshots and recovery

- Before each normal replacement, the last valid `notes.json` is atomically retained as `notes.previous.json`.
- If `notes.json` is unreadable, its exact bytes are preserved under `backups/`. A valid previous snapshot is restored when available; an unreadable previous snapshot is also preserved exactly.
- Recovery adds a visible notice note. With no valid snapshot, that notice is the only note in a fresh store.
- Only a brand-new store receives an initial empty note. A valid empty store represents intentional user state and remains empty across restarts.

## Window lifecycle

- Expanded dimensions remain canonical while a note is collapsed. Rust performs fold and unfold resizing, and clamps an expanded note to the display containing its collapsed header.
- Rust applies the durable pin state to both the native floating-window level and macOS window collection behavior. Pinned notes can join other applications' Stage Manager sets, while unpinned notes retain the app's normal transient behavior.
- macOS title-bar drags are anchored by Rust to the target native window and current cursor position; frontend code only initiates the drag gesture.
- New notes use the focused note as their anchor and persist the closest edge-aligned position that fits within its monitor work area without overlapping another note.
- Rust owns a live physical-geometry index for every open note, seeded at window creation and synchronously updated by native move, resize, and destruction events plus programmatic geometry changes. Debounced geometry-only saves persist that cache without rewriting unchanged durable values; editor saves flush content and final cached geometry together. Notes have no durable grouping relationship, and creating, dragging, resizing, folding, closing, restoring, or relaunching one note never repositions another. Legacy `linked_stack` metadata is accepted on load but ignored and omitted by the next normal save.
- Arrange Notes on This Side Below Current Note (Cmd+Shift+L or any note's vertical-arrangement button) is a one-time layout command. The selected note's center chooses the left or right half of its current monitor work area; only active notes whose centers are on that same half of that monitor are arranged. The selected note remains fixed, while matching notes retain their current sizes, keep top-to-bottom then left-to-right order, and move below it at its left edge with a 12px gap. No later action reflows that arrangement.
- Keyboard shortcut help uses a dedicated non-note window toggled by Help → Keyboard Shortcuts or Cmd+/. Esc also closes it. The help window is excluded from note ordering and coordinated note-save shutdown.
- Cmd+Shift+H toggles the visibility of all open note windows without changing their durable note state; restoring the notes returns focus to the previously focused note when it is still open.
- Cmd+Shift+R reopens any missing active note window, resets every active note into a visible cascade inside the primary display's work area, and persists the recovered positions without reopening archived notes or changing note sizes.
- Beginner installation starts with `scripts/bootstrap-macos.sh`, which prepares Apple's compiler tools, clones or updates `~/StickyMD`, and delegates to the root `install.sh`. The root installer checks Apple Silicon, installs checksum-verified official Node and Rust toolchains under the ignored `.tools/` directory, installs locked project dependencies, and delegates safe app replacement to `scripts/install-macos.sh`. Neither script modifies a user's existing Node, Rust, or shell configuration. Before requesting the running app's final save, each install atomically replaces one bounded `notes.preinstall.json` safety snapshot beside the durable note store; rebuilds never accumulate installer backups.
- Each compiled app embeds the repository's full commit SHA as its build identity. Help → Update opens one dedicated window and is the only automatic trigger for an update check; closing and reopening it starts a fresh check. The window compares that SHA with GitHub's public `main` commit, treats any difference as an available update, and never presents a failed or unknown comparison as up to date.
- Starting an available update opens the fixed `~/StickyMD/scripts/update.command` wrapper in Terminal. The wrapper accepts no arguments and delegates to the existing source installer, which remains the sole owner of source refresh, building, final note saving, app replacement, rollback, and relaunch.
- Every native exit request is blocked by an explicit quit state machine until each note webview acknowledges a successful save; only the coordinator's final exit is permitted.
- Close requests target only the focused note webview; application-wide emits are reserved for coordinated save-on-quit.
- Closing a note archives it with a durable timestamp before closing its window. Archived notes remain recoverable for 30 days, are skipped at launch, and are purged from the current store on the first launch after they expire. Cmd+Shift+T restores the most recent archived note; Cmd+Shift+U atomically restores every archived note and reopens any active note whose window is missing.
- Duplicate launches and macOS Dock reopens reopen any missing active note windows before focusing them, creating a note only when durable state has no active notes.
- The saved autostart preference is reconciled against the launch agent on every release launch. Development builds disable login-item registration.
- Updates remain user-triggered source builds. Local bundles use ad-hoc signing; notarization and public distribution require a separate release decision.
