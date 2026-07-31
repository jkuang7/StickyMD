// @ts-nocheck -- Keep this Node-runner regression free of test-only packages.
import assert from "node:assert/strict";
import test from "node:test";

import { getSchema, type JSONContent } from "@tiptap/core";
import type { Node as ProseMirrorNode } from "@tiptap/pm/model";
import { EditorState, TextSelection } from "@tiptap/pm/state";

import {
  createEditorExtensions,
  selectedTaskItemPositions,
} from "../src/lib/editorExtensions.ts";

const schema = getSchema(createEditorExtensions());

function task(
  text: string,
  checked = false,
  children: JSONContent[] = [],
): JSONContent {
  return {
    type: "taskItem",
    attrs: { checked },
    content: [
      { type: "paragraph", content: [{ type: "text", text }] },
      ...(children.length > 0
        ? [{ type: "taskList", content: children }]
        : []),
    ],
  };
}

function doc(...content: JSONContent[]): ProseMirrorNode {
  return schema.nodeFromJSON({ type: "doc", content });
}

function textPosition(document: ProseMirrorNode, text: string): number {
  let result: number | undefined;
  document.descendants((node, position) => {
    if (result === undefined && node.isText && node.text === text) {
      result = position;
    }
  });
  assert.notEqual(result, undefined, `Could not find text ${JSON.stringify(text)}`);
  return result;
}

function selectedTexts(
  document: ProseMirrorNode,
  from: number,
  to: number,
): string[] {
  const state = EditorState.create({
    doc: document,
    selection: TextSelection.create(document, from, to),
  });
  return selectedTaskItemPositions(state).map((position) =>
    document.nodeAt(position).firstChild.textContent,
  );
}

const document = doc({
  type: "taskList",
  content: [
    task("buy milk", true),
    task("call the bank"),
    task("ship release", true, [task("write notes"), task("tag build", true)]),
    task("fff"),
  ],
});

test("a cursor inside a task item selects only that item", () => {
  const cursor = textPosition(document, "call the bank") + 2;
  assert.deepEqual(selectedTexts(document, cursor, cursor), [
    "call the bank",
  ]);
});

test("a cursor inside a nested task item leaves its parent unselected", () => {
  const cursor = textPosition(document, "tag build") + 2;
  assert.deepEqual(selectedTexts(document, cursor, cursor), ["tag build"]);
});

test("a selection spanning items covers every item whose text it touches", () => {
  const from = textPosition(document, "call the bank") + 2;
  const to = textPosition(document, "write notes") + 3;
  assert.deepEqual(selectedTexts(document, from, to), [
    "call the bank",
    "ship release",
    "write notes",
  ]);
});

test("a selection stops short of items it does not reach", () => {
  const from = textPosition(document, "buy milk") + 1;
  const to = textPosition(document, "call the bank") + 2;
  assert.deepEqual(selectedTexts(document, from, to), [
    "buy milk",
    "call the bank",
  ]);
});
