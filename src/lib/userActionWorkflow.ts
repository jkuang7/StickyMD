export type UserAction = "close" | "fold" | "unfold";

export type UserActionOutcome =
  | { status: "succeeded" }
  | { status: "busy"; message: string }
  | { status: "failed"; message: string };

interface SurfaceActionAdapter {
  close(): Promise<void>;
  setCollapsed(collapsed: boolean): Promise<void>;
}

export interface UserActionWorkflow {
  perform(action: UserAction): Promise<UserActionOutcome>;
}

type ResolveTarget = () =>
  | SurfaceActionAdapter
  | undefined
  | Promise<SurfaceActionAdapter | undefined>;

function failureMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * One action may mutate a surface at a time. Input received while an action is
 * running is rejected as busy instead of being queued against potentially stale
 * state. The selected target is resolved once and retained for the whole action.
 */
export function createUserActionWorkflow(
  resolveTarget: ResolveTarget,
): UserActionWorkflow {
  let busy = false;

  return {
    async perform(action) {
      if (busy) {
        return {
          status: "busy",
          message: "Another user action is already running",
        };
      }
      busy = true;

      try {
        const target = await resolveTarget();
        if (!target) throw new Error("No valid note or timer target");

        switch (action) {
          case "close":
            await target.close();
            return { status: "succeeded" };
          case "fold":
            await target.setCollapsed(true);
            return { status: "succeeded" };
          case "unfold":
            await target.setCollapsed(false);
            return { status: "succeeded" };
        }
      } catch (error) {
        return { status: "failed", message: failureMessage(error) };
      } finally {
        busy = false;
      }
    },
  };
}

export function createNoteActionAdapter(dependencies: {
  flushPendingContent(): Promise<unknown>;
  closeSurface(): Promise<unknown>;
  setSurfaceCollapsed(collapsed: boolean): Promise<unknown>;
}): SurfaceActionAdapter {
  return {
    async close() {
      await dependencies.flushPendingContent();
      await dependencies.closeSurface();
    },
    async setCollapsed(collapsed) {
      await dependencies.flushPendingContent();
      await dependencies.setSurfaceCollapsed(collapsed);
    },
  };
}

export function createTimerActionAdapter(dependencies: {
  closeSurface(): Promise<unknown>;
  setSurfaceCollapsed(collapsed: boolean): Promise<unknown>;
}): SurfaceActionAdapter {
  return {
    async close() {
      await dependencies.closeSurface();
    },
    async setCollapsed(collapsed) {
      await dependencies.setSurfaceCollapsed(collapsed);
    },
  };
}

/** Translate an input request and render exactly the workflow outcome. */
export function createUserActionInputAdapter(
  workflow: UserActionWorkflow,
  render: (outcome: UserActionOutcome) => void,
) {
  async function perform(action: UserAction) {
    const outcome = await workflow.perform(action);
    render(outcome);
    return outcome;
  }

  return {
    close: () => perform("close"),
    fold: () => perform("fold"),
    unfold: () => perform("unfold"),
  };
}
