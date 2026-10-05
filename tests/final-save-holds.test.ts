import assert from "node:assert/strict";
import test from "node:test";
import { createFinalSaveLock, runQuitSave, updateFinalSaveHolds } from "../src/lib/finalSaveHolds.ts";

test("close and quit hold typing until both attempts end", () => {
  let editable = true;
  const lock = createFinalSaveLock(value => { editable = value; });
  const releaseClose = lock.holdClose();
  lock.beginQuit(1);
  releaseClose();
  assert.equal(editable, false);
  assert.equal(lock.isCurrentQuit(1), true);
  lock.endQuit(1);
  assert.equal(editable, true);
});

test("quit failure releases every quit hold but preserves a slow close", () => {
  let editable = true;
  const lock = createFinalSaveLock(value => { editable = value; });
  const releaseClose = lock.holdClose();
  lock.beginQuit(1);
  lock.endQuit(1);
  assert.equal(lock.isCurrentQuit(1), false);
  assert.equal(editable, false);
  releaseClose();
  assert.equal(editable, true);
  releaseClose();
  assert.equal(editable, true);
});

test("attempt 1 fails, attempt 2 starts, and attempt 1's late notice leaves 2 held", () => {
  let editable = true;
  const lock = createFinalSaveLock(value => { editable = value; });
  assert.equal(lock.beginQuit(1), true);
  lock.endQuit(1);
  assert.equal(editable, true);
  assert.equal(lock.beginQuit(2), true);
  lock.endQuit(1);
  assert.equal(editable, false);
  assert.equal(lock.isCurrentQuit(1), false);
  assert.equal(lock.isCurrentQuit(2), true);
  assert.equal(lock.beginQuit(1), false);
  assert.equal(lock.beginQuit(2), false);
  lock.endQuit(2);
  assert.equal(editable, true);
});

test("failure notice arriving before its request prevents acquiring that stale hold", () => {
  const lock = createFinalSaveLock(() => undefined);
  lock.endQuit(1);
  assert.equal(lock.beginQuit(1), false);
  assert.equal(lock.beginQuit(2), true);
});

for (const result of ["success", "failure"]) {
  test(`late quit save ${result} is ignored after its attempt ends`, async () => {
    let finish!: () => void;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    const lock = createFinalSaveLock(() => undefined);
    const reports: unknown[] = [];
    const first = runQuitSave(lock, 1, {
      async save() { await pending; if (result === "failure") throw new Error("old failure"); },
      async confirm(attempt) { reports.push(["confirm", attempt]); },
      async fail(attempt) { reports.push(["fail", attempt]); },
    });
    lock.endQuit(1);
    lock.beginQuit(2);
    finish();
    await first;
    assert.deepEqual(reports, []);
    assert.equal(lock.isCurrentQuit(2), true);
  });
}

test("quit locks before snapshot and stays locked after confirmation until the attempt ends", async () => {
  let editable = true;
  const lock = createFinalSaveLock(value => { editable = value; });
  const reports: number[] = [];
  await runQuitSave(lock, 1, {
    async save() { assert.equal(editable, false); },
    async confirm(attempt) { reports.push(attempt); },
    async fail() { assert.fail("Save succeeded"); },
  });
  assert.deepEqual(reports, [1]);
  assert.equal(editable, false);
  lock.endQuit(1);
  assert.equal(editable, true);
});


test("the pure hold rule leaves a newer quit and a close unchanged by an old notice", () => {
  const state = { closes: 1, latestQuit: 2, quit: 2 };
  assert.deepEqual(updateFinalSaveHolds(state, { type: "quit-end", attempt: 1 }), state);
  assert.deepEqual(state, { closes: 1, latestQuit: 2, quit: 2 });
  assert.deepEqual(updateFinalSaveHolds(state, { type: "quit-end", attempt: 2 }), {
    closes: 1, latestQuit: 2, quit: null,
  });
});

test("a failed quit reports once and the retry snapshots current content", async () => {
  let editable = true;
  const lock = createFinalSaveLock(value => { editable = value; });
  let content = "before failure";
  const snapshots: string[] = [];
  const failures: number[] = [];
  const confirmations: number[] = [];
  const actions = {
    async save() {
      assert.equal(editable, false);
      snapshots.push(content);
      if (snapshots.length === 1) throw new Error("read-only directory");
    },
    async fail(attempt: number) { failures.push(attempt); lock.endQuit(attempt); },
    async confirm(attempt: number) { confirmations.push(attempt); },
  };
  await runQuitSave(lock, 1, actions);
  assert.equal(editable, true);
  content = "edited after failure";
  await runQuitSave(lock, 2, actions);
  assert.deepEqual(snapshots, ["before failure", "edited after failure"]);
  assert.deepEqual(failures, [1]);
  assert.deepEqual(confirmations, [2]);
  assert.equal(editable, false);
});
