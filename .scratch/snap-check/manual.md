# Native snap verification — child #14

2026-10-05, macOS, current candidate source. Passed using a temporary Tauri
probe in the application crate, real Wry webview windows, and an isolated
`sticky-native-snap-<pid>` repository in the system temporary directory. The normal app setup was
bypassed; no existing user notes were read or written. Probe-only source changes
were restored before final tests and commit.

The probe created an independent pinned note and an unpinned note grouped with
another stored note. It called production `snap_note_window(Left, false)` on
both, waited one second for native movement, then called production settlement.
Both native frames and GeometryIndex matched the committed targets. The
unpinned note was detached. Loading a fresh NoteRepository from disk preserved
both positions and pin states. This checked repository reopening; a full
application relaunch was not performed.

The probe replaced the isolated repository's previous-snapshot file with a
directory and attempted a right snap. The production snap returned one save
failure. After native event delivery, the actual frame and GeometryIndex were
back at the prior saved position. Temporary app data was removed after the check.

Observed output:

```
NATIVE SNAP + SETTLEMENT + RELOAD: pinned=true target=(20, 300), native/live/durable agree, detached
NATIVE SNAP + SETTLEMENT + RELOAD: pinned=false target=(820, 300), native/live/durable agree, detached
FORCED SNAP SAVE FAILURE: Could not persist snapped note: Failed to atomically replace previous note snapshot: Failed to atomically replace storage: Is a directory (os error 21)
NATIVE ROLLBACK: native/live returned to saved snap; PASS
```

Full/partial targets in all directions and cross-monitor targeting are covered
by pure Rust tests; partial and cross-monitor native moves were not separately
performed. The same locked native transaction handles every computed target.
Rollback-failure aggregation is covered by the transaction's injected-effects
test. Immediate `outer_position()` reads can still report the old native frame;
the probe waits for the event loop before asserting frame changes.

Final verification: `npm run check` (zero errors/warnings), 44 user-action
workflow/production-adapter tests, 20 group tests, 23 note repository tests,
and 2 snap-target tests passed. Existing assertions were unchanged.

Review follow-up, 2026-10-05: repeated the native left snaps with isolated
production note loading and group restoration, exited, and started a second
process with the same repository. Native, live, and durable positions remained
(20, 300) for the pinned note and (340, 300) for the unpinned grouped note;
pin states were preserved and the grouped note remained detached. This used a
probe setup rather than the installed app's normal setup.

A forced save failure after externally moving the pinned note to (600, 300)
restored that native/live origin. Before correction, the next settlement also
saved (600, 300), incorrectly making the external move durable. After preserving
the pre-snap settlement tracking during rollback, the same native sequence kept
the saved position at (20, 300). The external-origin regression failed before
correction and passed afterward. A second regression preserves pending title-bar
drag detachment and delayed programmatic settlement through a failed snap.
Probe instrumentation was removed after verification.
