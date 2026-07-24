// @ts-nocheck -- Keep this Node-runner regression free of test-only packages.
import assert from "node:assert/strict";
import test from "node:test";

import { Editor } from "@tiptap/core";

import {
  createEditorExtensions,
  insertMarkdownClipboard,
  looksLikeMarkdown,
} from "../src/lib/editorExtensions.ts";

test("Markdown paste converts supported structure while ordinary prose stays plain", () => {
  const editor = new Editor({
    extensions: createEditorExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  let transactionCount = 0;
  let excludedFromHistory = false;
  editor.on("transaction", ({ transaction }) => {
    transactionCount += 1;
    excludedFromHistory ||= transaction.getMeta("addToHistory") === false;
  });

  const markdown = [
    "# Heading",
    "",
    "- parent",
    "  - child with **bold**",
    "",
    "---",
    "",
    "> quoted `code` and a [link](https://example.com)",
  ].join("\n");

  assert.equal(looksLikeMarkdown(markdown), true);
  assert.equal(
    insertMarkdownClipboard(editor, {
      getData(type) {
        if (type === "text/plain") return markdown;
        if (type === "text/html") {
          return "<p># Heading</p><p>- parent</p>";
        }
        return "";
      },
    }),
    true,
  );
  assert.deepEqual(
    editor.getJSON().content?.map((node) => node.type),
    ["heading", "bulletList", "horizontalRule", "blockquote"],
  );

  const bulletList = editor.getJSON().content?.[1];
  assert.equal(bulletList?.content?.[0].content?.[1].type, "bulletList");
  const json = JSON.stringify(editor.getJSON());
  assert.match(json, /"type":"bold"/);
  assert.match(json, /"type":"code"/);
  assert.match(json, /"type":"link"/);
  assert.equal(transactionCount, 1);
  assert.equal(excludedFromHistory, false);

  const prose = "Meet me at 3 p.m. and bring the project notes.";
  assert.equal(looksLikeMarkdown(prose), false);
  assert.equal(
    insertMarkdownClipboard(editor, {
      getData(type) {
        return type === "text/plain"
          ? prose
          : "<p>Meet me at 3 p.m. and bring the project notes.</p>";
      },
    }),
    false,
  );

  editor.destroy();
});
