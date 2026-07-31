<script lang="ts">
  import {
    mdiChevronDown,
    mdiChevronUp,
    mdiClose,
  } from "@mdi/js";
  import { Editor, type JSONContent } from "@tiptap/core";
  import { TextSelection } from "@tiptap/pm/state";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onDestroy, onMount } from "svelte";
  import {
    findNext,
    findPrev,
    getSearchState,
    SearchQuery,
    setSearchState,
    type SearchResult,
  } from "prosemirror-search";

  import { createEditorExtensions } from "./editorExtensions";
  import Icon from "./Icon.svelte";

  interface StickyInit {
    document: JSONContent;
    color: string;
  }

  let { onTitleChange = () => undefined, fontSize = 16 }: {
    onTitleChange?: (title: string) => void;
    fontSize?: number;
  } = $props();

  let shell: HTMLDivElement;
  let element: HTMLDivElement;
  let findInput = $state<HTMLInputElement>();
  let editor: Editor | undefined;
  let findOpen = $state(false);
  let findQuery = $state("");
  let currentMatch = $state(0);
  let totalMatches = $state(0);
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
    return shell.clientHeight + titlebarHeight;
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

  export function focus() {
    editor?.commands.focus();
  }

  function allMatches(
    searchEditor: Editor,
    query: SearchQuery,
  ): SearchResult[] {
    if (!query.valid) return [];

    const matches: SearchResult[] = [];
    const end = searchEditor.state.doc.content.size;
    for (let position = 0; position <= end; ) {
      const match = query.findNext(searchEditor.state, position, end);
      if (!match) break;
      matches.push(match);
      position = Math.max(match.to, position + 1);
    }
    return matches;
  }

  function syncFindState(searchEditor: Editor) {
    if (!findOpen) return;
    const query = getSearchState(searchEditor.state)?.query;
    if (!query?.valid) {
      currentMatch = 0;
      totalMatches = 0;
      return;
    }

    const matches = allMatches(searchEditor, query);
    const { from, to } = searchEditor.state.selection;
    totalMatches = matches.length;
    const activeIndex = matches.findIndex(
      (match) => match.from === from && match.to === to,
    );
    currentMatch = activeIndex < 0 ? 0 : activeIndex + 1;
  }

  function centerActiveMatch() {
    if (!editor) return;
    const searchEditor = editor;

    requestAnimationFrame(() => {
      if (
        !findOpen ||
        editor !== searchEditor ||
        searchEditor.isDestroyed
      ) {
        return;
      }

      const editable = element.querySelector<HTMLElement>(".tiptap");
      if (!editable) return;

      const { from, to } = searchEditor.state.selection;
      if (from === to) return;

      const start = searchEditor.view.coordsAtPos(from);
      const end = searchEditor.view.coordsAtPos(to, -1);
      const viewport = editable.getBoundingClientRect();
      const matchCenter =
        (Math.min(start.top, end.top) + Math.max(start.bottom, end.bottom)) /
        2;
      const viewportCenter = (viewport.top + viewport.bottom) / 2;
      const offset = matchCenter - viewportCenter;

      if (Math.abs(offset) > 1) editable.scrollTop += offset;
    });
  }

  function applyFindQuery(selectFirst: boolean) {
    if (!editor) return;
    const query = new SearchQuery({
      search: findQuery,
      caseSensitive: false,
      literal: true,
      regexp: false,
      wholeWord: false,
    });
    const matches = allMatches(editor, query);
    const { from, to } = editor.state.selection;
    const hasActiveMatch = matches.some(
      (match) => match.from === from && match.to === to,
    );
    let transaction = setSearchState(editor.state.tr, query);

    if (matches.length > 0 && (selectFirst || !hasActiveMatch)) {
      const first = matches[0];
      transaction = transaction.setSelection(
        TextSelection.create(transaction.doc, first.from, first.to),
      );
      transaction.scrollIntoView();
    }
    editor.view.dispatch(transaction);
    if (matches.length > 0) centerActiveMatch();
  }

  export function openFind() {
    if (!editor) return;
    if (!findOpen) {
      findOpen = true;
      applyFindQuery(false);
    } else if (totalMatches > 0) {
      centerActiveMatch();
    }
    requestAnimationFrame(() => {
      findInput?.focus();
      findInput?.select();
    });
  }

  function closeFind() {
    if (!editor) return;
    findOpen = false;
    editor.view.dispatch(
      setSearchState(
        editor.state.tr,
        new SearchQuery({ search: "", literal: true }),
      ),
    );
    currentMatch = 0;
    totalMatches = 0;
    requestAnimationFrame(() => editor?.commands.focus());
  }

  function updateFindQuery(event: Event) {
    findQuery = (event.currentTarget as HTMLInputElement).value;
    applyFindQuery(true);
  }

  function navigateFind(backward: boolean) {
    if (!editor || totalMatches === 0) return;
    const command = backward ? findPrev : findNext;
    if (command(editor.state, editor.view.dispatch, editor.view)) {
      centerActiveMatch();
    }
  }

  function handleFindKeydown(event: KeyboardEvent) {
    if (
      event.metaKey &&
      !event.ctrlKey &&
      !event.altKey &&
      !event.shiftKey &&
      event.key.toLowerCase() === "a"
    ) {
      event.preventDefault();
      event.stopPropagation();
      (event.currentTarget as HTMLInputElement).select();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeFind();
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    navigateFind(event.shiftKey);
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
          // macOS delivers Text Replacement through the same text-checking pass
          // as spelling and autocorrect, so either attribute disabled here also
          // disables replacement. Which substitutions actually run is chosen in
          // Rust (see src-tauri/src/text_checking.rs).
          autocorrect: "on",
          spellcheck: "true",
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
      onTransaction: ({ editor: transactionEditor }) => {
        syncFindState(transactionEditor);
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
  class="editor-shell"
  bind:this={shell}
  style:--note-font-size={`${fontSize}px`}
>
  <div class="editor" bind:this={element}></div>
  {#if findOpen}
    <div class="find-bar" role="search">
      <input
        bind:this={findInput}
        type="text"
        value={findQuery}
        oninput={updateFindQuery}
        onkeydown={handleFindKeydown}
        aria-label="Find in note"
        placeholder="Find"
        spellcheck="false"
        autocomplete="off"
      />
      <span class="find-count" aria-live="polite">
        {currentMatch} of {totalMatches}
      </span>
      <button
        disabled={totalMatches === 0}
        aria-label="Previous match"
        title="Previous match (Shift+Enter)"
        onmousedown={(event) => event.preventDefault()}
        onclick={() => navigateFind(true)}
      >
        <Icon path={mdiChevronUp} size={16} />
      </button>
      <button
        disabled={totalMatches === 0}
        aria-label="Next match"
        title="Next match (Enter)"
        onmousedown={(event) => event.preventDefault()}
        onclick={() => navigateFind(false)}
      >
        <Icon path={mdiChevronDown} size={16} />
      </button>
      <button
        aria-label="Close find"
        title="Close find (Escape)"
        onmousedown={(event) => event.preventDefault()}
        onclick={closeFind}
      >
        <Icon path={mdiClose} size={15} />
      </button>
    </div>
  {/if}
</div>

<style>
  .editor-shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    width: 100%;
  }

  .editor {
    flex: 1;
    min-height: 0;
    width: 100%;
  }

  .find-bar {
    align-items: center;
    background: rgba(255, 255, 255, 0.22);
    border-top: 1px solid rgba(0, 0, 0, 0.14);
    box-sizing: border-box;
    display: flex;
    flex: 0 0 31px;
    gap: 2px;
    padding: 4px 5px 4px 8px;
  }

  .find-bar input {
    background: rgba(255, 255, 255, 0.45);
    border: 1px solid rgba(0, 0, 0, 0.18);
    border-radius: 5px;
    box-sizing: border-box;
    color: inherit;
    flex: 1;
    font: 12px system-ui, sans-serif;
    height: 22px;
    min-width: 48px;
    outline: none;
    padding: 2px 6px;
  }

  .find-bar input:focus {
    border-color: rgba(0, 102, 255, 0.68);
    box-shadow: 0 0 0 1px rgba(0, 102, 255, 0.18);
  }

  .find-count {
    color: rgba(0, 0, 0, 0.55);
    flex: none;
    font: 11px/1 system-ui, sans-serif;
    min-width: 43px;
    text-align: center;
    white-space: nowrap;
  }

  .find-bar button {
    align-items: center;
    background: transparent;
    border: 0;
    border-radius: 4px;
    color: rgba(0, 0, 0, 0.68);
    display: flex;
    flex: none;
    height: 22px;
    justify-content: center;
    padding: 0;
    width: 22px;
  }

  .find-bar button:hover:not(:disabled) {
    background: rgba(0, 0, 0, 0.09);
  }

  .find-bar button:disabled {
    opacity: 0.28;
  }
</style>
