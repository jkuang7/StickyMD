export interface FinalSaveHolds {
  closes: number;
  latestQuit: number;
  quit: number | null;
}

type HoldEvent =
  | { type: "close-start" | "close-end" }
  | { type: "quit-start" | "quit-end"; attempt: number };

export function updateFinalSaveHolds(state: FinalSaveHolds, event: HoldEvent): FinalSaveHolds {
  switch (event.type) {
    case "close-start": return { ...state, closes: state.closes + 1 };
    case "close-end": return { ...state, closes: Math.max(0, state.closes - 1) };
    case "quit-start":
      return event.attempt > state.latestQuit
        ? { ...state, latestQuit: event.attempt, quit: event.attempt }
        : state;
    case "quit-end":
      return event.attempt >= state.latestQuit
        ? { ...state, latestQuit: event.attempt, quit: null }
        : state;
  }
}

export function createFinalSaveLock(setEditable: (editable: boolean) => void, initiallyEditable = true) {
  let state: FinalSaveHolds = { closes: 0, latestQuit: 0, quit: null };
  function apply(event: HoldEvent) {
    state = updateFinalSaveHolds(state, event);
    setEditable(initiallyEditable && state.closes === 0 && state.quit === null);
  }
  return {
    holdClose() {
      apply({ type: "close-start" });
      let released = false;
      return () => {
        if (released) return;
        released = true;
        apply({ type: "close-end" });
      };
    },
    beginQuit(attempt: number) {
      if (attempt <= state.latestQuit) return false;
      apply({ type: "quit-start", attempt });
      return true;
    },
    endQuit(attempt: number) { apply({ type: "quit-end", attempt }); },
    isCurrentQuit(attempt: number) { return state.quit === attempt; },
  };
}

export async function runQuitSave(
  lock: ReturnType<typeof createFinalSaveLock>,
  attempt: number,
  actions: {
    save(): Promise<void>;
    confirm(attempt: number): Promise<void>;
    fail(attempt: number, error: unknown): Promise<void>;
  },
) {
  if (!lock.beginQuit(attempt)) return;
  try {
    await actions.save();
  } catch (error) {
    if (lock.isCurrentQuit(attempt)) await actions.fail(attempt, error);
    return;
  }
  if (lock.isCurrentQuit(attempt)) await actions.confirm(attempt);
}
