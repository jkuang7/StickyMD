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
