import assert from "node:assert/strict";
import test from "node:test";
import { createEditorSaveQueue } from "../src/lib/editorSave.ts";

test("an ordinary flush queued behind a color save sends no color", async () => {
  const calls: { command: string; args: Record<string, unknown> }[] = [];
  let finishColor!: () => void;
  const delayedColor = new Promise<void>((resolve) => { finishColor = resolve; });
  const save = createEditorSaveQueue(async (command, args) => {
    calls.push({ command, args });
    if (calls.length === 1) await delayedColor;
  });
  const selectedDocument = { type: "doc", content: [] };
  const laterDocument = { type: "doc", content: [{ type: "paragraph" }] };
  const colorSave = save(selectedDocument, "#81b7dd");
  const ordinarySave = save(laterDocument);
  await delayedTurn();
  assert.deepEqual(calls, [{ command: "save_note", args: {
    document: selectedDocument, color: "#81b7dd",
  } }]);
  finishColor();
  await Promise.all([colorSave, ordinarySave]);
  assert.deepEqual(calls[1], { command: "save_note", args: { document: laterDocument } });
});

function delayedTurn() {
  return new Promise<void>((resolve) => setImmediate(resolve));
}


test("a failed save does not prevent the next ordinary save", async () => {
  const calls: Record<string, unknown>[] = [];
  const save = createEditorSaveQueue(async (_command, args) => {
    calls.push(args);
    if (calls.length === 1) throw new Error("save failed");
  });
  const failed = save({ type: "doc" }, "#81b7dd");
  const retry = save({ type: "doc", content: [] });
  await assert.rejects(failed, /save failed/);
  await retry;
  assert.deepEqual(calls[1], { document: { type: "doc", content: [] } });
});

test("a quit save whose attempt ended while queued is skipped", async () => {
  const calls: Record<string, unknown>[] = [];
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => { finish = resolve; });
  const save = createEditorSaveQueue(async (_command, args) => {
    calls.push(args);
    await pending;
  });
  let current = true;
  const first = save({ type: "doc" });
  const quit = save({ type: "doc", content: [] }, undefined, () => current);
  await delayedTurn();
  current = false;
  finish();
  await Promise.all([first, quit]);
  assert.equal(calls.length, 1);
});
