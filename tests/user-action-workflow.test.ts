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
