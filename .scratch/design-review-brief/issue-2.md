<!-- design-review-brief:v1 -->
# Design review brief

Source specification: https://github.com/jkuang7/StickyMD/issues/2

## Summary

Required safety steps live in individual titlebar and menu handlers instead of inside the shared action. Relink makes the problem concrete: the shortcut can rearrange windows without the confirmation or explicit note save performed by the titlebar.

The proposed workflow puts those safeguards behind one interface while leaving notes, timers, groups, native windows, and the quit coordinator in charge of their existing state.

### Before, safeguards depend on the caller

```text
Note titlebar  → confirm → flush note → rearrange
Timer titlebar → confirm ─────────────→ rearrange
Menu/shortcut ────────────────────────→ rearrange
```

### After, callers inherit the safeguards

```text
Note titlebar  ─┐
Timer titlebar ─┼→ shared workflow → ask for confirmation
Menu/shortcut ─┘                      ├─ The user cancels, so nothing changes.
                                      └─ The user continues.
                                           ├─ A note saves pending edits.
                                           └─ A timer skips the save.
                                                    │
                                                    ▼
                                             Rearrange the windows.
```

The mutation is already shared; its preparation is not. Moving policy into the workflow fixes the entry-path discrepancy and prevents new callers from reconstructing save, confirmation, and targeting rules. A missing explicit flush breaks the intended ordering guarantee, but does not itself prove edits were lost.

Evidence: [note titlebar](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src/routes/%2Bpage.svelte#L81-L127), [timer titlebar](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src/lib/Timer.svelte#L296-L365), and [menu dispatch](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src-tauri/src/menu.rs#L310-L339).

## Behavior

### Relink stops before mutation after cancellation or a failed save

```text
Capture the target once.
  ▼
Ask for confirmation.
  ├─ If the user cancels, finish without changing anything.
  ▼
Flush pending edits when the target is a note.
  ├─ If the save fails, stop before moving a window.
  ▼
Run the existing group operation.
  → Move the windows.
  → Save their positions.
  → Save the group membership.
  └─ After a failure, restore the prior window and saved state.
```

Linked-group membership remains owned by the note/layout store. The workflow coordinates prerequisites; the existing group operation retains layout and rollback behavior ([current relink implementation](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src-tauri/src/groups.rs#L667-L776)).

### Other actions share safeguards but keep their current results

- Closing a note saves it, compacts later group members, keeps its archive slot, archives it, and closes the window. Closing a timer removes its group slot and deletes the timer.
- Folding a note starts with a save. Folding a timer does not. Group reflow still moves only later visible members.
- Pinning a note starts with a save. Both note and timer pinning then update the native window, the repository, and the AeroSpace registry. A later failure triggers rollback.
- Quitting remains outside this workflow. Its coordinator still waits for every note's final save.

Evidence: [grouped close](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src-tauri/src/groups.rs#L1196-L1370), [fold/reflow](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src-tauri/src/groups.rs#L896-L1047), [note pin](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src-tauri/src/commands.rs#L249-L282), and [quit](https://github.com/jkuang7/StickyMD/blob/764c3da6f12b71a43d5bb69723b6a5f8e59dba55/src-tauri/src/lib.rs#L171-L190).

## Ownership

### The workflow owns ordering; existing modules keep state

```text
Titlebar ──────────────────────────┐
                                  ▼
Menu → focused-target selection → target webview
                                  │
                                  ▼
                         user-action workflow
                         Keeps the selected target.
                         Rejects overlapping work.
                         Confirms and saves when required.
                         Waits for Rust and returns the result.
                                  │
              ┌───────────────────┼───────────────────┐
              ▼                   ▼                   ▼
       Note and timer       Group and layout      Native window
       repositories         repository            operations
       save each surface.   saves membership.     change size and pin.
```

The webview continues to own the live editor; Rust continues to own durable note and timer state. This is a deep module because callers stop carrying the distributed ordering protocol, while the workflow does not become a competing source of truth.

## Decisions needed

### The spec still needs six answers

```text
The spec still needs six answers
├─ It keeps one captured target when focus changes.
│  It does not say whether a titlebar action starts with the clicked window
│  or whichever window currently has focus.
├─ It requires a successful note save before mutation.
│  It does not say how the action ends if the webview closes or never replies.
├─ It assigns repeated input to the workflow.
│  It does not say whether to reject, combine, or queue another request,
│  or whether that rule applies to one window, one group, or the whole app.
├─ It requires rollback after a later mutation fails.
│  It does not say which state the app trusts if rollback also fails.
├─ It includes link, close, fold, pin, and "note-only actions."
│  It does not name those additional note actions.
└─ It returns one result to each titlebar or menu handler.
   It does not say which failures the user must see or whether cancellation is silent.
```

If the webview closes or never confirms the save, the workflow cannot safely run the requested action. The issue must say when that request fails. The implementation can choose the event names and message format.

Rollback can also fail. At that point, the native window, repository, and AeroSpace registry may disagree. The issue must say which value the app trusts and what a retry should repair.

The other questions set the workflow's inputs and results. Targeting and scope determine which commands it accepts. The repeated-input rule determines which requests it rejects while work is in progress. The error rule determines what menu and titlebar tests must observe.

Once the issue answers all six questions, tickets can define the workflow interface and its tests. Relink should come first because it exercises targeting, confirmation, note saving, cancellation, and the existing group operation. Later tickets can move close, fold, settings, pin, and the named note actions.

<details>
<summary>Grounding</summary>

- Inspected local HEAD: `22d39235c71f1cc7a5c0b33741dc55e5e6a04ae6`; code links use identical production files at `origin/main` `764c3da6f12b71a43d5bb69723b6a5f8e59dba55`.
- User-owned uncommitted edits in `PLOT.md` and `src-tauri/src/groups.rs` concern pinned-window geometry settlement, not these entry paths; they were not modified.
- `PLOT.md` is the architecture source. No repository context map, ADRs, `AGENTS.md`, or `CLAUDE.md` exist.
- Issue body SHA-256: `738fad008a4171fc55f84cc916567cb28b45792c36408d88e1f8034d553b0249`.
- This brief explains and challenges the specification. It is not approval or implementation evidence.

</details>
