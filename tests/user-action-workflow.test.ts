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
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("close"), {
    status: "failed",
    message: "save unavailable",
  });
  assert.deepEqual(events, ["flush"]);
});

test("lifecycle and compensation failures stay one workflow failure", async () => {
  let attempts = 0;
  const target = createNoteActionAdapter({
    flushPendingContent: async () => undefined,
    closeSurface: async () => {
      attempts += 1;
      throw new Error("close failed; rollback also failed");
    },
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
  });
  const workflow = createUserActionWorkflow(() => target);

  assert.deepEqual(await workflow.perform("close"), { status: "succeeded" });
  assert.deepEqual(events, ["stop-delete-and-close"]);
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
  });
  const second = createTimerActionAdapter({
    closeSurface: async () => events.push("second close"),
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
  });
  const workflow = createUserActionWorkflow(() => target);
  const first = workflow.perform("close");
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(await workflow.perform("close"), { status: "busy" });
  assert.equal(closes, 1);
  release();
  assert.deepEqual(await first, { status: "succeeded" });
});

test("menu and webview input adapters render the workflow's one outcome", async () => {
  for (const entry of ["menu", "webview"]) {
    const rendered: unknown[] = [];
    const workflow = createUserActionWorkflow(() =>
      createTimerActionAdapter({
        closeSurface: async () => {
          throw new Error(`${entry} close failed`);
        },
      }),
    );
    const adapter = createUserActionInputAdapter(workflow, (outcome) => {
      rendered.push(outcome);
    });

    const outcome = await adapter.close();
    assert.deepEqual(outcome, {
      status: "failed",
      message: `${entry} close failed`,
    });
    assert.deepEqual(rendered, [outcome]);
  }
});
