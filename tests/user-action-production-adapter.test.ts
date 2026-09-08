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

function noteBindings(events, failures = new Map()) {
  return {
    async invoke(command, args) {
      events.push({ step: "invoke", command, args });
      if (failures.has(command)) throw new Error(failures.get(command));
    },
    async flushPendingContent(color) {
      events.push({ step: "flush", color });
      if (failures.has("flush")) throw new Error(failures.get("flush"));
    },
    prepareToCollapse() {
      events.push({ step: "prepare-to-collapse" });
    },
    displayColor(color) {
      events.push({ step: "display-color", color });
    },
  };
}

function timerBindings(events, failures = new Map()) {
  return {
    async invoke(command, args) {
      events.push({ step: "invoke", command, args });
      if (failures.has(command)) throw new Error(failures.get(command));
    },
  };
}

test("native menu and accelerator payloads use the production note commands", async () => {
  const events = [];
  const outcomes = [];
  const listener = memoryListener();
  const input = createNoteUserActionInput(
    noteBindings(events),
    async () => true,
    (outcome) => outcomes.push(outcome),
  );
  const unlisten = await registerUserActionRequestListener(listener.target, input);

  assert.equal(USER_ACTION_REQUEST_EVENT, "user_action_requested");
  assert.deepEqual([...listener.listeners.keys()], [USER_ACTION_REQUEST_EVENT]);
  await listener.dispatch("close");
  await listener.dispatch("relink");
  await listener.dispatch({ type: "set-color", color: "#65a65b" });
  await listener.dispatch({ type: "change-font-size", increase: false });
  await listener.dispatch({ type: "snap", direction: "Up", partial: false });
  await listener.dispatch({ type: "snap", direction: "Right", partial: true });

  assert.deepEqual(events, [
    { step: "flush", color: undefined },
    { step: "invoke", command: "close_window", args: undefined },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "link_windows_on_this_side_below_current_window",
      args: undefined,
    },
    { step: "flush", color: "#65a65b" },
    { step: "display-color", color: "#65a65b" },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "change_font_size",
      args: { increase: false },
    },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "snap_window",
      args: { direction: "Up", partial: false },
    },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "snap_window",
      args: { direction: "Right", partial: true },
    },
  ]);
  assert.deepEqual(outcomes, Array(6).fill({ status: "succeeded" }));

  unlisten();
  assert.equal(listener.listeners.size, 0);
});

test("invalid native payloads render failure without reaching a command", async () => {
  const events = [];
  const outcomes = [];
  const listener = memoryListener();
  const input = createNoteUserActionInput(
    noteBindings(events),
    async () => true,
    (outcome) => outcomes.push(outcome),
  );
  await registerUserActionRequestListener(listener.target, input);

  await listener.dispatch({ type: "snap", direction: "North", partial: false });

  assert.deepEqual(events, []);
  assert.deepEqual(outcomes, [
    { status: "failed", message: "Invalid user action request" },
  ]);
});

test("titlebar inputs use the production note commands", async () => {
  const events = [];
  const outcomes = [];
  const input = createNoteUserActionInput(
    noteBindings(events),
    async () => true,
    (outcome) => outcomes.push(outcome),
  );

  await input.fold();
  await input.unfold();
  await input.pin();
  await input.unpin();
  await input.relink();

  assert.deepEqual(events, [
    { step: "flush", color: undefined },
    { step: "prepare-to-collapse" },
    {
      step: "invoke",
      command: "set_collapsed",
      args: { collapsed: true },
    },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "set_collapsed",
      args: { collapsed: false },
    },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "set_note_always_on_top",
      args: { alwaysOnTop: true },
    },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "set_note_always_on_top",
      args: { alwaysOnTop: false },
    },
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "link_windows_on_this_side_below_current_window",
      args: undefined,
    },
  ]);
  assert.deepEqual(outcomes, Array(5).fill({ status: "succeeded" }));
});

test("timer inputs use production commands directly and reject note-only payloads", async () => {
  const events = [];
  const outcomes = [];
  const listener = memoryListener();
  const input = createTimerUserActionInput(
    timerBindings(events),
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
    { step: "invoke", command: "close_window", args: undefined },
    {
      step: "invoke",
      command: "set_collapsed",
      args: { collapsed: true },
    },
    {
      step: "invoke",
      command: "set_timer_always_on_top",
      args: { alwaysOnTop: true },
    },
  ]);
  assert.deepEqual(outcomes, [
    { status: "succeeded" },
    { status: "succeeded" },
    { status: "succeeded" },
    { status: "cancelled" },
    { status: "failed", message: "The focused surface is not a note" },
  ]);
});

test("every production note command failure is rendered as one outcome", async () => {
  const cases = [
    { failedStep: "flush", perform: (input) => input.close() },
    { failedStep: "close_window", perform: (input) => input.close() },
    { failedStep: "set_collapsed", perform: (input) => input.fold() },
    { failedStep: "set_note_always_on_top", perform: (input) => input.pin() },
    {
      failedStep: "link_windows_on_this_side_below_current_window",
      perform: (input) => input.relink(),
    },
    {
      failedStep: "change_font_size",
      perform: (input) => input.changeFontSize(true),
    },
    {
      failedStep: "snap_window",
      perform: (input) => input.snap("Down", true),
    },
  ];

  for (const { failedStep, perform } of cases) {
    const events = [];
    const outcomes = [];
    const message = `${failedStep} failed`;
    const input = createNoteUserActionInput(
      noteBindings(events, new Map([[failedStep, message]])),
      async () => true,
      (outcome) => outcomes.push(outcome),
    );

    assert.deepEqual(await perform(input), { status: "failed", message });
    assert.deepEqual(outcomes, [{ status: "failed", message }]);
    if (failedStep === "flush") {
      assert.deepEqual(events, [{ step: "flush", color: undefined }]);
    }
  }
});

test("cancellation and transaction compensation failures retain one outcome", async () => {
  const events = [];
  const outcomes = [];
  const message =
    "registry failed; compensation failed: durable rollback failed";
  const input = createNoteUserActionInput(
    noteBindings(events, new Map([["set_note_always_on_top", message]])),
    async () => false,
    (outcome) => outcomes.push(outcome),
  );

  assert.deepEqual(await input.relink(), { status: "cancelled" });
  assert.deepEqual(await input.pin(), { status: "failed", message });
  assert.deepEqual(events, [
    { step: "flush", color: undefined },
    {
      step: "invoke",
      command: "set_note_always_on_top",
      args: { alwaysOnTop: true },
    },
  ]);
  assert.deepEqual(outcomes, [
    { status: "cancelled" },
    { status: "failed", message },
  ]);
});

test("production listener renders busy without starting a second command", async () => {
  const events = [];
  const outcomes = [];
  const listener = memoryListener();
  let release;
  const bindings = noteBindings(events);
  bindings.flushPendingContent = () =>
    new Promise((resolve) => {
      events.push({ step: "flush" });
      release = resolve;
    });
  const input = createNoteUserActionInput(
    bindings,
    async () => true,
    (outcome) => outcomes.push(outcome),
  );
  await registerUserActionRequestListener(listener.target, input);

  const firstRequest = listener.dispatch("close");
  await new Promise((resolve) => setImmediate(resolve));
  await listener.dispatch("close");
  release();
  await firstRequest;

  assert.deepEqual(events, [
    { step: "flush" },
    { step: "invoke", command: "close_window", args: undefined },
  ]);
  assert.deepEqual(outcomes, [
    { status: "busy", message: "Another user action is already running" },
    { status: "succeeded" },
  ]);
});
