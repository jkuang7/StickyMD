// @ts-nocheck -- Keep this Node-runner regression free of test-only packages.
import assert from "node:assert/strict";
import test from "node:test";

import {
  USER_ACTION_REQUEST_EVENT,
  createNoteUserActionInput,
  createTimerUserActionInput,
  registerUserActionRequestListener,
} from "../src/lib/userActionWorkflow.ts";

function memoryListener() {
  const listeners = new Map();
  return {
    target: {
      async listen(name, listener) {
        listeners.set(name, listener);
        return () => listeners.delete(name);
      },
    },
    async dispatch(payload) {
      const listener = listeners.get(USER_ACTION_REQUEST_EVENT);
      assert.ok(listener, "production user-action listener must be registered");
      return listener({ payload });
    },
    listeners,
  };
}

function noteDependencies(events, failures = new Set()) {
  const run = async (step, detail = step) => {
    events.push(detail);
    if (failures.has(step)) throw new Error(`${step} failed`);
  };
  return {
    flushPendingContent: async (color) =>
      run("flush", color ? `flush:${color}` : "flush"),
    closeSurface: async () => run("close", "archive:group:native-close:focus"),
    setSurfaceCollapsed: async (collapsed) =>
      run("collapse", `${collapsed ? "fold" : "unfold"}:durable:native:group:focus`),
    setSurfacePinned: async (pinned) =>
      run("pin", `${pinned ? "pin" : "unpin"}:native:durable:registry:focus`),
    relinkSurface: async () => run("group", "group:native:focus"),
    setSurfaceColor: async (color) => run("color", `display:${color}`),
    changeSurfaceFontSize: async (increase) =>
      run("font", `${increase ? "font-up" : "font-down"}:durable:native:group:focus`),
    snapSurface: async (direction, partial) =>
      run("snap", `${partial ? "partial" : "full"}:${direction}:native:group:focus`),
  };
}

test("production listener maps every native menu and accelerator payload into the note workflow", async () => {
  const events = [];
  const outcomes = [];
  const listener = memoryListener();
  const input = createNoteUserActionInput(
    noteDependencies(events),
    async () => true,
    (outcome) => outcomes.push(outcome),
  );
  const unlisten = await registerUserActionRequestListener(listener.target, input);

  assert.deepEqual([...listener.listeners.keys()], [USER_ACTION_REQUEST_EVENT]);
  await listener.dispatch("close");
  await listener.dispatch("relink");
  await listener.dispatch({ type: "set-color", color: "#65a65b" });
  await listener.dispatch({ type: "change-font-size", increase: false });
  await listener.dispatch({ type: "snap", direction: "Up", partial: false });
  await listener.dispatch({ type: "snap", direction: "Right", partial: true });

  assert.deepEqual(events, [
    "flush",
    "archive:group:native-close:focus",
    "flush",
    "group:native:focus",
    "flush:#65a65b",
    "display:#65a65b",
    "flush",
    "font-down:durable:native:group:focus",
    "flush",
    "full:Up:native:group:focus",
    "flush",
    "partial:Right:native:group:focus",
  ]);
  assert.deepEqual(outcomes, Array(6).fill({ status: "succeeded" }));

  unlisten();
  assert.equal(listener.listeners.size, 0);
});

test("production titlebar input crosses the same note workflow seam", async () => {
  const events = [];
  const outcomes = [];
  const input = createNoteUserActionInput(
    noteDependencies(events),
    async () => true,
    (outcome) => outcomes.push(outcome),
  );

  await input.fold();
  await input.unfold();
  await input.pin();
  await input.unpin();
  await input.relink();

  assert.deepEqual(events, [
    "flush",
    "fold:durable:native:group:focus",
    "flush",
    "unfold:durable:native:group:focus",
    "flush",
    "pin:native:durable:registry:focus",
    "flush",
    "unpin:native:durable:registry:focus",
    "flush",
    "group:native:focus",
  ]);
  assert.deepEqual(outcomes, Array(5).fill({ status: "succeeded" }));
});

test("production timer listener keeps shared actions direct and rejects note-only payloads", async () => {
  const events = [];
  const outcomes = [];
  const listener = memoryListener();
  const input = createTimerUserActionInput(
    {
      closeSurface: async () => events.push("delete:group:native-close:focus"),
      setSurfaceCollapsed: async (collapsed) =>
        events.push(`${collapsed ? "fold" : "unfold"}:durable:native:group:focus`),
      setSurfacePinned: async (pinned) =>
        events.push(`${pinned ? "pin" : "unpin"}:native:durable:registry:focus`),
      relinkSurface: async () => events.push("group:native:focus"),
    },
    async () => false,
    (outcome) => outcomes.push(outcome),
  );
  await registerUserActionRequestListener(listener.target, input);

  await listener.dispatch("close");
  await input.fold();
  await input.pin();
  await listener.dispatch("relink");
  await listener.dispatch({ type: "snap", direction: "Left", partial: false });

  assert.deepEqual(events, [
    "delete:group:native-close:focus",
    "fold:durable:native:group:focus",
    "pin:native:durable:registry:focus",
  ]);
  assert.deepEqual(outcomes, [
    { status: "succeeded" },
    { status: "succeeded" },
    { status: "succeeded" },
    { status: "cancelled" },
    { status: "failed", message: "The focused surface is not a note" },
  ]);
});

test("production listener renders flush and downstream failures without later mutation", async () => {
  for (const failedStep of ["flush", "group", "pin", "snap"]) {
    const events = [];
    const outcomes = [];
    const listener = memoryListener();
    const input = createNoteUserActionInput(
      noteDependencies(events, new Set([failedStep])),
      async () => true,
      (outcome) => outcomes.push(outcome),
    );
    await registerUserActionRequestListener(listener.target, input);

    const payload =
      failedStep === "group"
        ? "relink"
        : failedStep === "pin"
          ? "pin"
          : failedStep === "snap"
            ? { type: "snap", direction: "Down", partial: true }
            : "close";
    await listener.dispatch(payload);

    assert.deepEqual(outcomes, [
      { status: "failed", message: `${failedStep} failed` },
    ]);
    if (failedStep === "flush") assert.deepEqual(events, ["flush"]);
  }
});

test("production seam renders missing targets, cancellation, and transaction compensation failures", async () => {
  const missingOutcomes = [];
  const missing = createNoteUserActionInput(
    () => undefined,
    async () => true,
    (outcome) => missingOutcomes.push(outcome),
  );
  assert.deepEqual(await missing.close(), {
    status: "failed",
    message: "No valid note or timer target",
  });
  assert.deepEqual(missingOutcomes, [
    { status: "failed", message: "No valid note or timer target" },
  ]);

  const events = [];
  const outcomes = [];
  const dependencies = noteDependencies(events);
  dependencies.setSurfacePinned = async () => {
    events.push("pin transaction");
    throw new Error("registry failed; compensation failed: durable rollback failed");
  };
  const input = createNoteUserActionInput(
    dependencies,
    async () => false,
    (outcome) => outcomes.push(outcome),
  );

  assert.deepEqual(await input.relink(), { status: "cancelled" });
  assert.deepEqual(await input.pin(), {
    status: "failed",
    message: "registry failed; compensation failed: durable rollback failed",
  });
  assert.deepEqual(events, ["flush", "pin transaction"]);
  assert.deepEqual(outcomes, [
    { status: "cancelled" },
    {
      status: "failed",
      message: "registry failed; compensation failed: durable rollback failed",
    },
  ]);
});

test("production listener preserves stable targeting and renders busy outcomes", async () => {
  const outcomes = [];
  const listener = memoryListener();
  let release;
  let focusedEvents = [];
  const first = noteDependencies(focusedEvents);
  first.flushPendingContent = () =>
    new Promise((resolve) => {
      focusedEvents.push("first flush");
      release = resolve;
    });
  const secondEvents = [];
  let focused = first;
  const input = createNoteUserActionInput(
    () => focused,
    async () => true,
    (outcome) => outcomes.push(outcome),
  );
  await registerUserActionRequestListener(listener.target, input);

  const firstRequest = listener.dispatch("close");
  await new Promise((resolve) => setImmediate(resolve));
  focused = noteDependencies(secondEvents);
  await listener.dispatch("close");
  release();
  await firstRequest;

  assert.deepEqual(focusedEvents, [
    "first flush",
    "archive:group:native-close:focus",
  ]);
  assert.deepEqual(secondEvents, []);
  assert.deepEqual(outcomes, [
    { status: "busy", message: "Another user action is already running" },
    { status: "succeeded" },
  ]);
});
