<script lang="ts">
  import { Editor, type JSONContent } from "@tiptap/core";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onDestroy, onMount } from "svelte";

  import { createEditorExtensions } from "./editorExtensions";

  interface StickyInit {
    document: JSONContent;
    color: string;
  }

  let { onTitleChange = () => undefined, fontSize = 16 }: {
    onTitleChange?: (title: string) => void;
    fontSize?: number;
  } = $props();

  let element: HTMLDivElement;
  let editor: Editor | undefined;
  let saveTimeout: number | undefined;
  let saveChain: Promise<void> = Promise.resolve();
  const unlisteners: UnlistenFn[] = [];
  const minimumWindowHeight = 80;
  const titlebarHeight = 24;

  function currentTitle(): string {
    if (!editor) return "Empty Note";
    const text = editor.state.doc.textBetween(
      0,
      editor.state.doc.content.size,
      "\n",
    );
    return text
      .split("\n")
      .map((line) => line.trim())
      .find(Boolean) ?? "Empty Note";
  }

  function intrinsicContentHeight(editable: HTMLElement): number {
    const previousHeight = editable.style.height;
    const previousMinHeight = editable.style.minHeight;
    const previousOverflowY = editable.style.overflowY;

    editable.style.height = "auto";
    editable.style.minHeight = "0";
    editable.style.overflowY = "hidden";
    const height = editable.scrollHeight;
    editable.style.height = previousHeight;
    editable.style.minHeight = previousMinHeight;
    editable.style.overflowY = previousOverflowY;

    return height;
  }

  export function currentWindowHeight(): number {
    return element.clientHeight + titlebarHeight;
  }

  export function currentContentHeight(): number | undefined {
    const editable = element.querySelector<HTMLElement>(".tiptap");
    if (!editable) return undefined;
    return intrinsicContentHeight(editable);
  }

  async function resizeWindow(targetHeight: number) {
    if (Math.abs(targetHeight - currentWindowHeight()) <= 1) return;
    await invoke("resize_note_height", { height: Math.round(targetHeight) });
  }

  export async function resizeForFontSize(
    baselineWindowHeight: number,
    baselineContentHeight: number,
  ) {
    const contentHeight = currentContentHeight();
    if (contentHeight === undefined) return;
    const targetHeight = Math.max(
      minimumWindowHeight,
      baselineWindowHeight + contentHeight - baselineContentHeight,
    );
    await resizeWindow(targetHeight);
  }

  export async function growToFit() {
    const contentHeight = currentContentHeight();
    if (contentHeight === undefined) return;
    await resizeWindow(
      Math.max(currentWindowHeight(), contentHeight + titlebarHeight),
    );
  }

  function queueSave(delay = 2_000) {
    if (saveTimeout !== undefined) window.clearTimeout(saveTimeout);
    saveTimeout = window.setTimeout(() => void flushSave(), delay);
  }

  export async function flushSave() {
    if (saveTimeout !== undefined) {
      window.clearTimeout(saveTimeout);
      saveTimeout = undefined;
    }
    if (!editor) return;

    const snapshot = editor.getJSON();
    const color = document.body.style.backgroundColor;
    const save = saveChain
      .catch(() => undefined)
      .then(async () => {
        await invoke("save_note", {
          document: snapshot,
          color,
        });
      });
    saveChain = save;
    await save;
  }

  export function removeSelection() {
    editor?.commands.blur();
  }

  export function focus() {
    editor?.commands.focus();
  }

  onMount(async () => {
    const init = (window as typeof window & { __STICKY_INIT__?: StickyInit })
      .__STICKY_INIT__;

    editor = new Editor({
      element,
      extensions: createEditorExtensions(),
      content: init?.document ?? { type: "doc", content: [{ type: "paragraph" }] },
      editorProps: {
        attributes: {
          autocorrect: "off",
          spellcheck: "false",
        },
        handleDOMEvents: {
          focusin: (view, event) => {
            const target = event.target;
            if (
              !(target instanceof HTMLInputElement) ||
              target.type !== "checkbox"
            ) {
              return false;
            }
            view.focus();
            return true;
          },
        },
      },
      onUpdate: () => {
        onTitleChange(currentTitle());
        queueSave();
      },
    });

    document.body.style.backgroundColor = init?.color || "#fff9b1";
    onTitleChange(currentTitle());
    requestAnimationFrame(() => editor?.commands.focus());

    unlisteners.push(
      await listen("save_request", () => flushSave()),
      await listen("flush_before_quit", async () => {
        try {
          await flushSave();
          await invoke("acknowledge_quit");
        } catch (error) {
          console.error("Could not save note before quitting", error);
        }
      }),
    );

  });

  onDestroy(() => {
    if (saveTimeout !== undefined) window.clearTimeout(saveTimeout);
    unlisteners.forEach((unlisten) => unlisten());
    editor?.destroy();
  });
</script>

<div
  class="editor"
  bind:this={element}
  style:--note-font-size={`${fontSize}px`}
></div>

<style>
  .editor {
    height: 100%;
    width: 100%;
  }
</style>
