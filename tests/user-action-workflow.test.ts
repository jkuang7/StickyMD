// @ts-nocheck -- Keep this Node-runner regression free of test-only packages.
import assert from "node:assert/strict";
import test from "node:test";

import {
  createNoteActionAdapter,
  createTimerActionAdapter,
  createUserActionInputAdapter,
  createUserActionWorkflow,
} from "../src/lib/userActionWorkflow.ts";

test("note close flushes before lifecycle mutation", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => events.push("flush"),
    closeSurface: async () => events.push("archive-and-close"),
    setSurfaceCollapsed: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("close"), { status: "succeeded" });
  assert.deepEqual(events, ["flush", "archive-and-close"]);
});

test("failed note flush prevents lifecycle mutation", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => {
      events.push("flush");
      throw new Error("save unavailable");
    },
    closeSurface: async () => events.push("archive-and-close"),
    setSurfaceCollapsed: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("close"), {
    status: "failed",
    message: "save unavailable",
  });
  assert.deepEqual(events, ["flush"]);
});

test("note fold and unfold flush before durable and native mutation", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => events.push("flush"),
    closeSurface: async () => events.push("archive-and-close"),
    setSurfaceCollapsed: async (collapsed) =>
      events.push(collapsed ? "fold" : "unfold"),
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("fold"), { status: "succeeded" });
  assert.deepEqual(await workflow.perform("unfold"), { status: "succeeded" });
  assert.deepEqual(events, ["flush", "fold", "flush", "unfold"]);
});

test("failed note flush prevents fold and unfold mutation", async () => {
  const mutations: boolean[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => {
      throw new Error("save unavailable");
    },
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async (collapsed) => mutations.push(collapsed),
  });
  const workflow = createUserActionWorkflow(() => target);

  for (const action of ["fold", "unfold"]) {
    assert.deepEqual(await workflow.perform(action), {
      status: "failed",
      message: "save unavailable",
    });
  }
  assert.deepEqual(mutations, []);
});

test("lifecycle and compensation failures stay one workflow failure", async () => {
  let attempts = 0;
  const target = createNoteActionAdapter({
    flushPendingContent: async () => undefined,
    closeSurface: async () => {
      attempts += 1;
      throw new Error("close failed; rollback also failed");
    },
    setSurfaceCollapsed: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  const expected = {
    status: "failed",
    message: "close failed; rollback also failed",
  };
  assert.deepEqual(await workflow.perform("close"), expected);
  assert.deepEqual(await workflow.perform("close"), expected);
  assert.equal(attempts, 2, "failure must release the workflow busy state");
});

test("timer close skips note persistence", async () => {
  const events: string[] = [];
  const target = createTimerActionAdapter({
    closeSurface: async () => events.push("stop-delete-and-close"),
    setSurfaceCollapsed: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("close"), { status: "succeeded" });
  assert.deepEqual(events, ["stop-delete-and-close"]);
});

test("timer fold and unfold mutate directly without note persistence", async () => {
  const collapsedStates: boolean[] = [];
  const target = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async (collapsed) =>
      collapsedStates.push(collapsed),
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("fold"), { status: "succeeded" });
  assert.deepEqual(await workflow.perform("unfold"), { status: "succeeded" });
  assert.deepEqual(collapsedStates, [true, false]);
});

test("the resolved target stays fixed while an asynchronous close is running", async () => {
  const events: string[] = [];
  let releaseFlush!: () => void;
  let focused = createNoteActionAdapter({
    flushPendingContent: () =>
      new Promise<void>((resolve) => {
        events.push("first flush");
        releaseFlush = resolve;
      }),
    closeSurface: async () => events.push("first close"),
    setSurfaceCollapsed: async () => undefined,
  });
  const second = createTimerActionAdapter({
    closeSurface: async () => events.push("second close"),
    setSurfaceCollapsed: async () => undefined,
  });
  let resolutions = 0;
  const workflow = createUserActionWorkflow(() => {
    resolutions += 1;
    return focused;
  });

  const close = workflow.perform("close");
  await new Promise((resolve) => setImmediate(resolve));
  focused = second;
  releaseFlush();

  assert.deepEqual(await close, { status: "succeeded" });
  assert.equal(resolutions, 1);
  assert.deepEqual(events, ["first flush", "first close"]);
});

test("missing targets and duplicate input do not mutate a surface", async () => {
  const missing = createUserActionWorkflow(() => undefined);
  assert.deepEqual(await missing.perform("close"), {
    status: "failed",
    message: "No valid note or timer target",
  });

  let release!: () => void;
  let closes = 0;
  const target = createTimerActionAdapter({
    closeSurface: () =>
      new Promise<void>((resolve) => {
        closes += 1;
        release = resolve;
      }),
    setSurfaceCollapsed: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);
  const first = workflow.perform("close");
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(await workflow.perform("close"), {
    status: "busy",
    message: "Another user action is already running",
  });
  assert.equal(closes, 1);
  release();
  assert.deepEqual(await first, { status: "succeeded" });
});

test("the shared input adapter maps close and renders every workflow outcome", async () => {
  const outcomes = [
    { status: "succeeded" },
    { status: "busy", message: "Another user action is already running" },
    { status: "failed", message: "close failed" },
  ];
  const actions: string[] = [];
  const rendered: unknown[] = [];
  const workflow = {
    async perform(action) {
      actions.push(action);
      return outcomes[actions.length - 1];
    },
  };
  const adapter = createUserActionInputAdapter(workflow, (outcome) => {
    rendered.push(outcome);
  });

  assert.deepEqual(await adapter.close(), outcomes[0]);
  assert.deepEqual(await adapter.close(), outcomes[1]);
  assert.deepEqual(await adapter.close(), outcomes[2]);
  assert.deepEqual(actions, ["close", "close", "close"]);
  assert.deepEqual(rendered, outcomes);
});

test("the shared input adapter routes fold and unfold through the workflow", async () => {
  const actions: string[] = [];
  const workflow = {
    async perform(action) {
      actions.push(action);
      return { status: "succeeded" };
    },
  };
  const adapter = createUserActionInputAdapter(workflow, () => undefined);

  assert.deepEqual(await adapter.fold(), { status: "succeeded" });
  assert.deepEqual(await adapter.unfold(), { status: "succeeded" });
  assert.deepEqual(actions, ["fold", "unfold"]);
});

test("fold shares the workflow busy policy with every other action", async () => {
  let release!: () => void;
  const target = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: () =>
      new Promise<void>((resolve) => {
        release = resolve;
      }),
  });
  const workflow = createUserActionWorkflow(() => target);
  const fold = workflow.perform("fold");
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(await workflow.perform("close"), {
    status: "busy",
    message: "Another user action is already running",
  });
  release();
  assert.deepEqual(await fold, { status: "succeeded" });
});

test("note pin and unpin flush before the pin transaction", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => events.push("flush"),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async (pinned) =>
      events.push(pinned ? "pin" : "unpin"),
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("pin"), { status: "succeeded" });
  assert.deepEqual(await workflow.perform("unpin"), { status: "succeeded" });
  assert.deepEqual(events, ["flush", "pin", "flush", "unpin"]);
});

test("failed note flush prevents every pin representation from changing", async () => {
  const mutations: boolean[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => {
      throw new Error("save unavailable");
    },
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async (pinned) => mutations.push(pinned),
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("pin"), {
    status: "failed",
    message: "save unavailable",
  });
  assert.deepEqual(mutations, []);
});

test("timer pin skips note persistence and returns one transaction failure", async () => {
  const requested: boolean[] = [];
  const target = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    async setSurfacePinned(pinned) {
      requested.push(pinned);
      throw new Error("registry failed; durable rollback failed");
    },
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("unpin"), {
    status: "failed",
    message: "registry failed; durable rollback failed",
  });
  assert.deepEqual(requested, [false]);
});

test("the shared input adapter routes pin outcomes through its renderer", async () => {
  const actions: string[] = [];
  const rendered: unknown[] = [];
  const workflow = {
    async perform(action) {
      actions.push(action);
      return { status: "succeeded" };
    },
  };
  const adapter = createUserActionInputAdapter(workflow, (outcome) =>
    rendered.push(outcome),
  );

  assert.deepEqual(await adapter.pin(), { status: "succeeded" });
  assert.deepEqual(await adapter.unpin(), { status: "succeeded" });
  assert.deepEqual(actions, ["pin", "unpin"]);
  assert.deepEqual(rendered, [
    { status: "succeeded" },
    { status: "succeeded" },
  ]);
});

test("pin shares stable targeting and the workflow busy policy", async () => {
  let release!: () => void;
  const events: string[] = [];
  const first = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: () =>
      new Promise<void>((resolve) => {
        events.push("first pin");
        release = resolve;
      }),
  });
  const second = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => events.push("second pin"),
  });
  let focused = first;
  const workflow = createUserActionWorkflow(() => focused);

  const pin = workflow.perform("pin");
  await new Promise((resolve) => setImmediate(resolve));
  focused = second;
  assert.deepEqual(await workflow.perform("unpin"), {
    status: "busy",
    message: "Another user action is already running",
  });
  release();

  assert.deepEqual(await pin, { status: "succeeded" });
  assert.deepEqual(events, ["first pin"]);
});

test("relink resolves once, confirms, then flushes a note before mutation", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => events.push("flush"),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => events.push("relink"),
  });
  const workflow = createUserActionWorkflow(
    () => {
      events.push("resolve");
      return target;
    },
    async () => {
      events.push("confirm");
      return true;
    },
  );

  assert.deepEqual(await workflow.perform("relink"), { status: "succeeded" });
  assert.deepEqual(events, ["resolve", "confirm", "flush", "relink"]);
});

test("cancelled relink is a successful no-op before note persistence", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => events.push("flush"),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => events.push("relink"),
  });
  const workflow = createUserActionWorkflow(
    () => target,
    async () => {
      events.push("confirm");
      return false;
    },
  );

  assert.deepEqual(await workflow.perform("relink"), { status: "cancelled" });
  assert.deepEqual(events, ["confirm"]);
});

test("failed note flush prevents an accepted relink mutation", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => {
      events.push("flush");
      throw new Error("save unavailable");
    },
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => events.push("relink"),
  });
  const workflow = createUserActionWorkflow(() => target, async () => true);

  assert.deepEqual(await workflow.perform("relink"), {
    status: "failed",
    message: "save unavailable",
  });
  assert.deepEqual(events, ["flush"]);
});

test("timer relink confirms and mutates without note persistence", async () => {
  const events: string[] = [];
  const target = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => events.push("relink"),
  });
  const workflow = createUserActionWorkflow(
    () => target,
    async () => {
      events.push("confirm");
      return true;
    },
  );

  assert.deepEqual(await workflow.perform("relink"), { status: "succeeded" });
  assert.deepEqual(events, ["confirm", "relink"]);
});

test("relink retains its target and stays busy throughout confirmation", async () => {
  const events: string[] = [];
  let releaseConfirmation!: (accepted: boolean) => void;
  const first = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => events.push("first relink"),
  });
  const second = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => events.push("second relink"),
  });
  let focused = first;
  let resolutions = 0;
  const workflow = createUserActionWorkflow(
    () => {
      resolutions += 1;
      return focused;
    },
    () =>
      new Promise<boolean>((resolve) => {
        events.push("confirm");
        releaseConfirmation = resolve;
      }),
  );

  const relink = workflow.perform("relink");
  await new Promise((resolve) => setImmediate(resolve));
  focused = second;
  assert.deepEqual(await workflow.perform("relink"), {
    status: "busy",
    message: "Another user action is already running",
  });
  releaseConfirmation(true);

  assert.deepEqual(await relink, { status: "succeeded" });
  assert.equal(resolutions, 1);
  assert.deepEqual(events, ["confirm", "first relink"]);
});

test("missing relink targets fail without confirmation or mutation", async () => {
  let confirmations = 0;
  const workflow = createUserActionWorkflow(
    () => undefined,
    async () => {
      confirmations += 1;
      return true;
    },
  );

  assert.deepEqual(await workflow.perform("relink"), {
    status: "failed",
    message: "No valid note or timer target",
  });
  assert.equal(confirmations, 0);
});

test("the shared input adapter routes relink success, cancellation, and failure", async () => {
  const outcomes = [
    { status: "succeeded" },
    { status: "cancelled" },
    { status: "failed", message: "relink failed" },
  ];
  const actions: string[] = [];
  const rendered: unknown[] = [];
  const workflow = {
    async perform(action) {
      actions.push(action);
      return outcomes[actions.length - 1];
    },
  };
  const adapter = createUserActionInputAdapter(workflow, (outcome) =>
    rendered.push(outcome),
  );

  assert.deepEqual(await adapter.relink(), outcomes[0]);
  assert.deepEqual(await adapter.relink(), outcomes[1]);
  assert.deepEqual(await adapter.relink(), outcomes[2]);
  assert.deepEqual(actions, ["relink", "relink", "relink"]);
  assert.deepEqual(rendered, outcomes);
});

test("note color saves the selected color and current document before displaying it", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async (color) =>
      events.push(`save current document with ${color}`),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
    setSurfaceColor: async (color) => events.push(`display ${color}`),
    changeSurfaceFontSize: async () => undefined,
    snapSurface: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(
    await workflow.perform({ type: "set-color", color: "#81b7dd" }),
    { status: "succeeded" },
  );
  assert.deepEqual(events, [
    "save current document with #81b7dd",
    "display #81b7dd",
  ]);
});

test("failed color persistence leaves the displayed color unchanged", async () => {
  const displayed: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => {
      throw new Error("save unavailable");
    },
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
    setSurfaceColor: async (color) => displayed.push(color),
    changeSurfaceFontSize: async () => undefined,
    snapSurface: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(
    await workflow.perform({ type: "set-color", color: "#81b7dd" }),
    { status: "failed", message: "save unavailable" },
  );
  assert.deepEqual(displayed, []);
});

test("note font-size and snap actions flush before durable or native mutation", async () => {
  const events: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => events.push("flush"),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
    setSurfaceColor: async () => undefined,
    changeSurfaceFontSize: async (increase) =>
      events.push(increase ? "font up" : "font down"),
    snapSurface: async (direction, partial) =>
      events.push(`${partial ? "partial" : "full"} ${direction}`),
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(
    await workflow.perform({ type: "change-font-size", increase: true }),
    { status: "succeeded" },
  );
  assert.deepEqual(
    await workflow.perform({ type: "snap", direction: "Left", partial: false }),
    { status: "succeeded" },
  );
  assert.deepEqual(
    await workflow.perform({ type: "snap", direction: "Down", partial: true }),
    { status: "succeeded" },
  );
  assert.deepEqual(events, [
    "flush",
    "font up",
    "flush",
    "full Left",
    "flush",
    "partial Down",
  ]);
});

test("a failed note save prevents font-size and snap mutation", async () => {
  const mutations: string[] = [];
  const target = createNoteActionAdapter({
    flushPendingContent: async () => {
      throw new Error("save unavailable");
    },
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
    setSurfaceColor: async () => undefined,
    changeSurfaceFontSize: async () => mutations.push("font"),
    snapSurface: async () => mutations.push("snap"),
  });
  const workflow = createUserActionWorkflow(() => target);

  for (const action of [
    { type: "change-font-size", increase: false },
    { type: "snap", direction: "Up", partial: false },
  ]) {
    assert.deepEqual(await workflow.perform(action), {
      status: "failed",
      message: "save unavailable",
    });
  }
  assert.deepEqual(mutations, []);
});

test("note-only actions reject timer targets through the workflow seam", async () => {
  const mutations: string[] = [];
  const target = createTimerActionAdapter({
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
  });
  const workflow = createUserActionWorkflow(() => target);

  for (const action of [
    { type: "set-color", color: "#81b7dd" },
    { type: "change-font-size", increase: true },
    { type: "snap", direction: "Right", partial: true },
  ]) {
    assert.deepEqual(await workflow.perform(action), {
      status: "failed",
      message: "The focused surface is not a note",
    });
  }
  assert.deepEqual(mutations, []);
});

test("note-only actions reject utility or missing targets before saving", async () => {
  let resolutions = 0;
  const workflow = createUserActionWorkflow(() => {
    resolutions += 1;
    return undefined;
  });

  assert.deepEqual(
    await workflow.perform({ type: "set-color", color: "#81b7dd" }),
    { status: "failed", message: "No valid note or timer target" },
  );
  assert.equal(resolutions, 1);
});

test("note-only actions retain the resolved target while saving", async () => {
  const events: string[] = [];
  let releaseSave!: () => void;
  const first = createNoteActionAdapter({
    flushPendingContent: () =>
      new Promise<void>((resolve) => {
        events.push("first save");
        releaseSave = resolve;
      }),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
    setSurfaceColor: async () => undefined,
    changeSurfaceFontSize: async () => events.push("first font"),
    snapSurface: async () => undefined,
  });
  const second = createNoteActionAdapter({
    flushPendingContent: async () => events.push("second save"),
    closeSurface: async () => undefined,
    setSurfaceCollapsed: async () => undefined,
    setSurfacePinned: async () => undefined,
    relinkSurface: async () => undefined,
    setSurfaceColor: async () => undefined,
    changeSurfaceFontSize: async () => events.push("second font"),
    snapSurface: async () => undefined,
  });
  let focused = first;
  let resolutions = 0;
  const workflow = createUserActionWorkflow(() => {
    resolutions += 1;
    return focused;
  });

  const action = workflow.perform({ type: "change-font-size", increase: true });
  await new Promise((resolve) => setImmediate(resolve));
  focused = second;
  assert.deepEqual(
    await workflow.perform({ type: "snap", direction: "Left", partial: false }),
    { status: "busy", message: "Another user action is already running" },
  );
  releaseSave();

  assert.deepEqual(await action, { status: "succeeded" });
  assert.equal(resolutions, 1);
  assert.deepEqual(events, ["first save", "first font"]);
});

test("the shared input adapter maps color, font-size, and both snap variants", async () => {
  const actions: unknown[] = [];
  const rendered: unknown[] = [];
  const workflow = {
    async perform(action) {
      actions.push(action);
      return { status: "succeeded" };
    },
  };
  const adapter = createUserActionInputAdapter(workflow, (outcome) =>
    rendered.push(outcome),
  );

  await adapter.setColor("#65a65b");
  await adapter.changeFontSize(false);
  await adapter.snap("Up", false);
  await adapter.snap("Right", true);

  assert.deepEqual(actions, [
    { type: "set-color", color: "#65a65b" },
    { type: "change-font-size", increase: false },
    { type: "snap", direction: "Up", partial: false },
    { type: "snap", direction: "Right", partial: true },
  ]);
  assert.deepEqual(rendered, Array(4).fill({ status: "succeeded" }));
});
