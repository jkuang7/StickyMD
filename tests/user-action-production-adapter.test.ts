// @ts-nocheck -- Keep this Node-runner regression free of test-only packages.
import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
import { createEditorExtensions, holdEditorTyping, editorFinalSaveLock } from "../src/lib/editorExtensions.ts";

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
    holdTyping: () => () => undefined,
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

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

function createEditor() {
  return new Editor({
    element: null,
    extensions: createEditorExtensions(),
    content: { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "accepted" }] }] },
  });
}

test("close holds the editor before its snapshot until the close ends", async () => {
  const editor = createEditor();
  const save = deferred();
  const close = deferred();
  const closing = deferred();
  let updates = 0;
  editor.on("update", () => { updates += 1; });
  const input = createNoteUserActionInput({
    holdTyping: () => holdEditorTyping(editor),
    async flushPendingContent() {
      assert.equal(editor.isEditable, false);
      assert.equal(editor.getText(), "accepted");
      await save.promise;
    },
    async invoke(command) {
      assert.equal(command, "close_window");
      assert.equal(editor.isEditable, false);
      closing.resolve();
      await close.promise;
    },
    prepareToCollapse() {},
    displayColor() {},
  }, async () => true, () => undefined);
  try {
    const pending = input.close();
    await Promise.resolve();
    assert.equal(editor.isEditable, false);
    assert.deepEqual(await input.close(), { status: "busy", message: "Another user action is already running" });
    assert.equal(editor.isEditable, false);
    save.resolve();
    await closing.promise;
    assert.equal(editor.isEditable, false);
    close.resolve();
    assert.deepEqual(await pending, { status: "succeeded" });
    assert.equal(editor.isEditable, true);
    assert.equal(updates, 0);
  } finally {
    editor.destroy();
  }
});

for (const failedStep of ["save", "close"]) {
  test(`${failedStep} failure releases typing before displaying the failure`, async () => {
    const editor = createEditor();
    const outcomes: unknown[] = [];
    let closeCalls = 0;
    const input = createNoteUserActionInput({
      holdTyping: () => holdEditorTyping(editor),
      async flushPendingContent() {
        assert.equal(editor.isEditable, false);
        if (failedStep === "save") throw new Error("save failed");
      },
      async invoke() {
        closeCalls += 1;
        assert.equal(editor.isEditable, false);
        throw new Error("close failed");
      },
      prepareToCollapse() {},
      displayColor() {},
    }, async () => true, (outcome) => {
      assert.equal(editor.isEditable, true);
      outcomes.push(outcome);
    });
    try {
      const expected = { status: "failed", message: `${failedStep} failed` };
      assert.deepEqual(await input.close(), expected);
      assert.deepEqual(outcomes, [expected]);
      assert.equal(closeCalls, failedStep === "save" ? 0 : 1);
      assert.equal(editor.isEditable, true);
    } finally {
      editor.destroy();
    }
  });
}

test("other action saves leave typing enabled", async () => {
  const editor = createEditor();
  let holds = 0;
  let saves = 0;
  const input = createNoteUserActionInput({
    holdTyping() { holds += 1; return holdEditorTyping(editor); },
    async flushPendingContent() {
      saves += 1;
      assert.equal(editor.isEditable, true);
    },
    async invoke() { assert.equal(editor.isEditable, true); },
    prepareToCollapse() {},
    displayColor() {},
  }, async () => true, () => undefined);
  try {
    const outcomes = [
      await input.fold(),
      await input.unfold(),
      await input.pin(),
      await input.unpin(),
      await input.relink(),
      await input.setColor("#65a65b"),
      await input.changeFontSize(true),
      await input.snap("Left", false),
    ];
    assert.ok(outcomes.every((outcome) => outcome.status === "succeeded"));
    assert.equal(holds, 0);
    assert.equal(saves, 8);
  } finally {
    editor.destroy();
  }
});


test("a failed quit releases the editor unless a close still holds it", () => {
  const editor = createEditor();
  try {
    const lock = editorFinalSaveLock(editor);
    const releaseClose = holdEditorTyping(editor);
    lock.beginQuit(1);
    lock.endQuit(1);
    assert.equal(editor.isEditable, false);
    releaseClose();
    assert.equal(editor.isEditable, true);
    lock.beginQuit(2);
    lock.endQuit(1);
    assert.equal(editor.isEditable, false);
    lock.endQuit(2);
    assert.equal(editor.isEditable, true);
  } finally {
    editor.destroy();
  }
});
