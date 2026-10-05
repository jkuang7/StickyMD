import type { Event } from "@tauri-apps/api/event";

export type UserAction =
  | "close"
  | "fold"
  | "unfold"
  | "pin"
  | "unpin"
  | "relink"
  | { type: "set-color"; color: string }
  | { type: "change-font-size"; increase: boolean }
  | { type: "snap"; direction: SnapDirection; partial: boolean };

export type SnapDirection = "Up" | "Down" | "Left" | "Right";

export type UserActionOutcome =
  | { status: "succeeded" }
  | { status: "cancelled" }
  | { status: "busy"; message: string }
  | { status: "failed"; message: string };

interface SurfaceActionAdapter {
  close(): Promise<void>;
  setCollapsed(collapsed: boolean): Promise<void>;
  setPinned(pinned: boolean): Promise<void>;
  relink(): Promise<void>;
  setColor(color: string): Promise<void>;
  changeFontSize(increase: boolean): Promise<void>;
  snap(direction: SnapDirection, partial: boolean): Promise<void>;
}

export interface UserActionWorkflow {
  perform(action: UserAction): Promise<UserActionOutcome>;
}

type ResolveTarget = () =>
  | SurfaceActionAdapter
  | undefined
  | Promise<SurfaceActionAdapter | undefined>;

const RELINK_CONFIRMATION = "Are you sure you want to link these windows?";

function failureMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isUserAction(action: unknown): action is UserAction {
  if (
    action === "close" ||
    action === "fold" ||
    action === "unfold" ||
    action === "pin" ||
    action === "unpin" ||
    action === "relink"
  ) {
    return true;
  }
  if (!action || typeof action !== "object" || !("type" in action)) {
    return false;
  }

  const request = action as Record<string, unknown>;
  if (request.type === "set-color") return typeof request.color === "string";
  if (request.type === "change-font-size") {
    return typeof request.increase === "boolean";
  }
  return (
    request.type === "snap" &&
    (request.direction === "Up" ||
      request.direction === "Down" ||
      request.direction === "Left" ||
      request.direction === "Right") &&
    typeof request.partial === "boolean"
  );
}

/**
 * One action may mutate a surface at a time. Input received while an action is
 * running is rejected as busy instead of being queued against potentially stale
 * state. The selected target is resolved once and retained for the whole action.
 */
export function createUserActionWorkflow(
  resolveTarget: ResolveTarget,
  confirmRelink: (message: string) => Promise<boolean> = async () => {
    throw new Error("Relink confirmation is not configured");
  },
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

        if (typeof action !== "string") {
          switch (action.type) {
            case "set-color":
              await target.setColor(action.color);
              return { status: "succeeded" };
            case "change-font-size":
              await target.changeFontSize(action.increase);
              return { status: "succeeded" };
            case "snap":
              await target.snap(action.direction, action.partial);
              return { status: "succeeded" };
          }
        }

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
          case "pin":
            await target.setPinned(true);
            return { status: "succeeded" };
          case "unpin":
            await target.setPinned(false);
            return { status: "succeeded" };
          case "relink":
            if (!(await confirmRelink(RELINK_CONFIRMATION))) {
              return { status: "cancelled" };
            }
            await target.relink();
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

export function createNoteActionAdapter(
  dependencies: NoteActionDependencies,
): SurfaceActionAdapter {
  return {
    async close() {
      const releaseTyping = dependencies.holdTyping();
      try {
        await dependencies.flushPendingContent();
        await dependencies.closeSurface();
      } finally {
        releaseTyping();
      }
    },
    async setCollapsed(collapsed) {
      await dependencies.flushPendingContent();
      await dependencies.setSurfaceCollapsed(collapsed);
    },
    async setPinned(pinned) {
      await dependencies.flushPendingContent();
      await dependencies.setSurfacePinned(pinned);
    },
    async relink() {
      await dependencies.flushPendingContent();
      await dependencies.relinkSurface();
    },
    async setColor(color) {
      await dependencies.flushPendingContent(color);
      await dependencies.setSurfaceColor(color);
    },
    async changeFontSize(increase) {
      await dependencies.flushPendingContent();
      await dependencies.changeSurfaceFontSize(increase);
    },
    async snap(direction, partial) {
      await dependencies.flushPendingContent();
      await dependencies.snapSurface(direction, partial);
    },
  };
}

export function createTimerActionAdapter(
  dependencies: TimerActionDependencies,
): SurfaceActionAdapter {
  return {
    async close() {
      await dependencies.closeSurface();
    },
    async setCollapsed(collapsed) {
      await dependencies.setSurfaceCollapsed(collapsed);
    },
    async setPinned(pinned) {
      await dependencies.setSurfacePinned(pinned);
    },
    async relink() {
      await dependencies.relinkSurface();
    },
    async setColor() {
      throw new Error("The focused surface is not a note");
    },
    async changeFontSize() {
      throw new Error("The focused surface is not a note");
    },
    async snap() {
      throw new Error("The focused surface is not a note");
    },
  };
}

/** Translate an input request and render exactly the workflow outcome. */
export function createUserActionInputAdapter(
  workflow: UserActionWorkflow,
  render: (outcome: UserActionOutcome) => void,
) {
  async function perform(action: UserAction) {
    const outcome = isUserAction(action)
      ? await workflow.perform(action)
      : { status: "failed" as const, message: "Invalid user action request" };
    render(outcome);
    return outcome;
  }

  return {
    perform,
    close: () => perform("close"),
    fold: () => perform("fold"),
    unfold: () => perform("unfold"),
    pin: () => perform("pin"),
    unpin: () => perform("unpin"),
    relink: () => perform("relink"),
    setColor: (color: string) => perform({ type: "set-color", color }),
    changeFontSize: (increase: boolean) =>
      perform({ type: "change-font-size", increase }),
    snap: (direction: SnapDirection, partial: boolean) =>
      perform({ type: "snap", direction, partial }),
  };
}

export const USER_ACTION_REQUEST_EVENT = "user_action_requested";

interface NoteActionDependencies {
  holdTyping(): () => void;
  flushPendingContent(color?: string): Promise<unknown>;
  closeSurface(): Promise<unknown>;
  setSurfaceCollapsed(collapsed: boolean): Promise<unknown>;
  setSurfacePinned(pinned: boolean): Promise<unknown>;
  relinkSurface(): Promise<unknown>;
  setSurfaceColor(color: string): Promise<unknown>;
  changeSurfaceFontSize(increase: boolean): Promise<unknown>;
  snapSurface(direction: SnapDirection, partial: boolean): Promise<unknown>;
}

interface TimerActionDependencies {
  closeSurface(): Promise<unknown>;
  setSurfaceCollapsed(collapsed: boolean): Promise<unknown>;
  setSurfacePinned(pinned: boolean): Promise<unknown>;
  relinkSurface(): Promise<unknown>;
}

type ConfirmRelink = (message: string) => Promise<boolean>;
type RenderOutcome = (outcome: UserActionOutcome) => void;

type Invoke = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;

export interface NoteUserActionBindings {
  invoke: Invoke;
  holdTyping(): () => void;
  flushPendingContent(color?: string): Promise<unknown>;
  prepareToCollapse(): void;
  displayColor(color: string): void;
}

export interface TimerUserActionBindings {
  invoke: Invoke;
}

/** Compose the exact workflow and note adapter used by production callers. */
export function createNoteUserActionInput(
  bindings: NoteUserActionBindings,
  confirmRelink: ConfirmRelink,
  render: RenderOutcome,
) {
  const target = createNoteActionAdapter({
    holdTyping: bindings.holdTyping,
    flushPendingContent: bindings.flushPendingContent,
    closeSurface: () => bindings.invoke("close_window"),
    async setSurfaceCollapsed(collapsed) {
      if (collapsed) bindings.prepareToCollapse();
      await bindings.invoke("set_collapsed", { collapsed });
    },
    setSurfacePinned: (pinned) =>
      bindings.invoke("set_note_always_on_top", { alwaysOnTop: pinned }),
    relinkSurface: () =>
      bindings.invoke("link_windows_on_this_side_below_current_window"),
    async setSurfaceColor(color) {
      bindings.displayColor(color);
    },
    changeSurfaceFontSize: (increase) =>
      bindings.invoke("change_font_size", { increase }),
    snapSurface: (direction, partial) =>
      bindings.invoke("snap_window", { direction, partial }),
  });
  const workflow = createUserActionWorkflow(() => target, confirmRelink);
  return createUserActionInputAdapter(workflow, render);
}

/** Compose the exact workflow and timer adapter used by production callers. */
export function createTimerUserActionInput(
  bindings: TimerUserActionBindings,
  confirmRelink: ConfirmRelink,
  render: RenderOutcome,
) {
  const target = createTimerActionAdapter({
    closeSurface: () => bindings.invoke("close_window"),
    setSurfaceCollapsed: (collapsed) =>
      bindings.invoke("set_collapsed", { collapsed }),
    setSurfacePinned: (pinned) =>
      bindings.invoke("set_timer_always_on_top", { alwaysOnTop: pinned }),
    relinkSurface: () =>
      bindings.invoke("link_windows_on_this_side_below_current_window"),
  });
  const workflow = createUserActionWorkflow(() => target, confirmRelink);
  return createUserActionInputAdapter(workflow, render);
}

interface UserActionEventTarget {
  listen<T>(
    event: string,
    handler: (event: Event<T>) => void | Promise<void>,
  ): Promise<() => void>;
}

/** Route native menu items and their accelerators through the production seam. */
export function registerUserActionRequestListener(
  target: UserActionEventTarget,
  input: ReturnType<typeof createUserActionInputAdapter>,
) {
  return target.listen<UserAction>(USER_ACTION_REQUEST_EVENT, async (event) => {
    await input.perform(event.payload);
  });
}
