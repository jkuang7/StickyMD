# Design review brief: put the safety rules inside the action

Draft for [StickyMD issue #2](https://github.com/jkuang7/StickyMD/issues/2). Proposed design, not implemented behavior.

## The problem: the same action has different safeguards

Sticky has note windows and timer windows. “Relink windows” arranges windows into a group beneath the selected window.

You can request this through a titlebar button or the menu/keyboard shortcut. Today, those routes do different things:

```text
Click a note’s link button
  → ask permission
  → save pending note edits
  → rearrange windows

Use the menu / shortcut
  → rearrange windows
```

The shortcut can rearrange windows without asking permission or explicitly waiting for pending note edits to be saved.

This happens even when actions run one at a time. It does not require a race condition. The missing save step breaks the intended save-before-action guarantee; it does not by itself prove that relinking loses edits.

## Why the code permits this

The window-rearranging operation is already shared. The safety rules around it are not.

A **caller** is the code handling a button click or menu selection. Today, each caller must know what to do before invoking the shared operation:

```text
CURRENT

Button handler owns:
  confirmation → saving ────┐
                           ├──→ shared rearranging operation
Menu handler ──────────────┘
```

Some rules are repeated across handlers; some routes omit them. The entire action is not implemented twice.

This is an **encapsulation problem**: the operation relies on callers to enforce its required safety steps.

It also creates **temporal coupling**: callers must know which operations to call and in what order.

For a developer entering the codebase, reading the shared operation is therefore insufficient. They must trace each caller to discover whether confirmation and saving happen before it.

## The change: make the shared action include its safeguards

```text
PROPOSED

Button handler ──┐
                 ├──→ shared relink action
Menu handler ───┘       │
                        └─ confirmation → saving → rearranging
```

The callers request “relink.” The shared action knows how to perform it safely, including skipping note-saving for timers.

For users, this makes the safeguards consistent across entry points.

For developers, there is one place to understand or change the sequence. A new shortcut inherits the safeguards instead of having to reproduce them.

Adding the missing checks to the menu could fix this particular discrepancy. The broader proposal applies the same ownership rule to close, fold, pin, and relink, so their callers stop carrying action policy.

## Follow one complete action: relink

The workflow captures the target once. A focus change while waiting must not redirect the action to another window.

Recommended sequence: ask permission before saving. This preserves the spec’s cancellation rule without triggering an action-driven durable save first. Make this ordering explicit in the spec.

```text
Button or menu requests relink
             │
             ▼
       Capture target
             ├─ invalid → stop; change nothing
             ▼
       Ask permission
             ├─ cancel → stop; change nothing
             ▼
    Save pending note edits
       (timer skips this)
             ├─ failure → stop; do not rearrange
             ▼
  Recheck the same target and
  resolve conflicts before mutation
             ├─ target no longer valid → stop
             ▼
    Existing relink operation
      move windows
           → save positions
           → save group membership
             ├─ failure → attempt recovery; report failure
             ▼
       Report completion
```

The no-change cancellation rule concerns effects of this action. Independent background activity, such as an ordinary editor autosave, may still occur.

Conflict handling and recovery after a failed rollback remain design decisions; the diagram does not establish those guarantees.

## Apply the same rule without erasing note/timer differences

The workflow owns the sequence. Existing operations retain their lifecycle and group behavior.

```text
CLOSE NOTE
  save pending edits
    → existing close operation
        archive note; retain its group slot
        compact remaining windows; close window

CLOSE TIMER
  existing close operation
    stop/delete timer; remove its group slot
    compact remaining windows; close window
```

These are lifecycle outcomes, not a replacement specification for the internal mutation order of grouped close.

```text
FOLD / UNFOLD
  note:  save → existing fold operation
  timer:        existing fold operation

PIN / UNPIN
  note:  save → existing pin sequence
  timer:        existing pin sequence

OPEN TIMER SETTINGS
  unfold successfully → show settings
```

A failed required note save must prevent the later lifecycle or window mutations.

## Pin shows why the action must also own recovery

One pin choice changes three representations:

```text
Native window     → Saved preference     → AeroSpace registry
floating now        restored on launch    workspace mirroring
```

If a later step fails, earlier steps may already have succeeded:

```text
Native changed → saving fails
                   └─ restore previous native state

Native changed → preference saved → registry update fails
                                      └─ restore previous
                                         saved + native state
```

Existing pin code already attempts this compensation. The workflow consolidates responsibility for the sequence and its outcome.

**Open question:** if restoration also fails, which state is authoritative, how is disagreement repaired, and what does the user see? “Roll back” alone does not answer this.

## What the workflow owns—and what it calls

```text
Entry handlers
  translate user input; present the result
        │
        ▼
UserActionWorkflow
  target selection
  confirmation and save policy
  action ordering and conflict handling
  coordination, recovery, and outcome
        │
        ├──→ Note webview: live editor contents
        ├──→ Rust repositories: saved notes and timers
        ├──→ Note/layout store: group membership
        └──→ Native operations: physical window changes
```

The workflow coordinates existing owners. It does not become another data store. Application quit keeps its existing coordinator, which waits for all required note saves before exit.

## How the spec handles the difficult cases

The spec establishes the safety rules and assigns them to the workflow. It does not settle every edge case or implementation mechanism. The distinction matters: an unanswered technical detail does not mean the safety rule is missing.

The answers below come from the spec’s [Implementation Decisions](https://github.com/jkuang7/StickyMD/issues/2#implementation-decisions), [User Stories](https://github.com/jkuang7/StickyMD/issues/2#user-stories), and [Testing Decisions](https://github.com/jkuang7/StickyMD/issues/2#testing-decisions).

**Can the action proceed while the editor is still saving?**

The spec says no: a required note save must complete before mutation, and a failed save must prevent close, fold, pin, and relink mutations. The editor remains in the webview; the workflow requests the save.

```text
Workflow requests save → editor saves → completion → mutation may begin
                                     └─ failure  → stop
```

**Remaining gap:** how completion reaches the workflow, and what happens if the editor closes or never responds. Message plumbing can be designed during implementation. Timeout and window-loss behavior must be defined so the action cannot hang indefinitely or proceed without a successful save; these are recommendations to complete the contract, not policies already specified.

**Can a focus change make the action affect the wrong window?**

The spec already requires selecting the target once and retaining it through asynchronous work. Invalid or missing targets fail before mutation.

```text
Capture window A → wait for confirmation/save → still act on A
                          focus moves to B ──→ does not retarget
```

**Remaining gap:** the initial selection rule for titlebars. Menu and shortcut actions explicitly use the focused surface, but the spec does not clearly say whether a titlebar always nominates its own window. Clarify this behavior; stable targeting after capture is already decided.

**What if the user requests another action while one is running?**

The spec assigns duplicate-action policy to the workflow and requires consistent handling. It does not choose the behavior.

```text
Close A is running → another action arrives
                      reject? combine? wait?
```

**Remaining gap:** choose the conflict policy and its scope. Two different windows may share a group, so protecting only repeated input on one window does not establish group safety. This affects user-visible behavior and the concurrency tests; it should be resolved as an explicit design decision.

**What if an action changes some state and then fails?**

The spec already requires compensation. For pinning, a failed durable update restores native state; a failed registry update restores durable and native state. It also requires one coherent outcome and treats cancellation as a successful no-op.

**Remaining gap:** what happens when compensation itself fails—not whether ordinary compensation is required. Define which state is authoritative, how disagreement is repaired, and how the failure is presented. These choices determine what the user can trust after an error; “restore everything” is not a complete policy for failed restoration.

**Which actions are actually included?**

The spec names link, close, fold/unfold, and pin/unpin. User story 20 also routes unfolding for timer settings through the workflow.

**Remaining gap:** the additional phrase “note-only actions that require durable-save or focused-note policy” is not an action list. Enumerate those actions before decomposing that portion of the work, so implementers do not independently expand or narrow the scope.

**Can quitting interrupt the save guarantee?**

The spec says the existing quit coordinator remains responsible for exit and must continue waiting for every required successful final save.

```text
Quit requested → coordinator awaits required saves → exit
```

**Remaining gap:** how a save already in progress participates—whether quit joins it or requires a separate final-save acknowledgement. That mechanism can be resolved during implementation if it preserves the coordinator’s guarantee and is tested with overlapping actions. Any choice that changes that guarantee requires a spec decision.

**What needs clarification versus implementation design?**

Clarify the extra action scope, titlebar targeting, conflicting-action behavior, and failed-recovery behavior in the spec. Define missing timeout/window-loss behavior as part of completing the save contract.

Implementation can choose the communication mechanism and quit/save integration under those contracts. These details need design and evidence, but they do not all require a separate human decision before tickets can exist.

## What implementation must demonstrate

Tests should invoke the same workflow interface used by the handlers and show that:

- Both entry paths require relink confirmation; cancellation causes no action-driven mutation.
- Failed note saves prevent close, fold, pin, and relink mutations; timers skip note saves.
- Focus changes cannot redirect an action to another target.
- Pin failures exercise compensation, including the chosen policy for failed compensation.
- Conflicting group actions preserve the agreed final state.
- Quit still waits for every required successful note-save acknowledgement.

## Evidence behind this draft

Inspected revision: `22d39235c71f1cc7a5c0b33741dc55e5e6a04ae6`.

- [Note handlers: confirmation, saving, and action calls](https://github.com/jkuang7/StickyMD/blob/22d39235c71f1cc7a5c0b33741dc55e5e6a04ae6/src/routes/%2Bpage.svelte#L81-L117)
- [Timer handlers: separate confirmation and action calls](https://github.com/jkuang7/StickyMD/blob/22d39235c71f1cc7a5c0b33741dc55e5e6a04ae6/src/lib/Timer.svelte#L296-L365)
- [Menu handlers: direct action dispatch](https://github.com/jkuang7/StickyMD/blob/22d39235c71f1cc7a5c0b33741dc55e5e6a04ae6/src-tauri/src/menu.rs#L310-L339)
- [Existing note pin compensation](https://github.com/jkuang7/StickyMD/blob/22d39235c71f1cc7a5c0b33741dc55e5e6a04ae6/src-tauri/src/commands.rs#L249-L282)

The checkout has unrelated uncommitted changes in `PLOT.md` and `src-tauri/src/groups.rs` concerning external window movement. This draft uses committed code as its evidence and makes no claim of implementation or new test execution.
