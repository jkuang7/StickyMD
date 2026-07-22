<script lang="ts">
  import {
    mdiClose,
    mdiLinkVariant,
    mdiPalette,
    mdiPin,
    mdiPinOff,
  } from "@mdi/js";
  import { invoke } from "@tauri-apps/api/core";
  import { webviewWindow } from "@tauri-apps/api";
  import { onDestroy, onMount } from "svelte";

  import Editor from "$lib/Editor.svelte";
  import Icon from "$lib/Icon.svelte";
  import ShortcutsHelp from "$lib/ShortcutsHelp.svelte";
  import Timer from "$lib/Timer.svelte";
  import Version from "$lib/Version.svelte";

  interface StickyInit {
    always_on_top?: boolean;
    collapsed?: boolean;
    font_size?: number;
  }

  interface TimerInit {
    id: string;
    elapsed_ms: number;
    running: boolean;
    reminder_interval_ms: number;
    alarm_at_ms: number;
    always_on_top: boolean;
    collapsed: boolean;
  }

  const colors = [
    "#fff9b1",
    "#81b7dd",
    "#65a65b",
    "#aad2ca",
    "#98c260",
    "#e1a1b1",
    "#b98cb3",
    "#f3c178",
    "#ef8d7f",
    "#c9b5e8",
    "#d1d5db",
  ];
  const appWindow = webviewWindow.getCurrentWebviewWindow();
  const shortcutsWindow = Boolean(
    (window as typeof window & { __SHORTCUTS__?: boolean }).__SHORTCUTS__,
  );
  const versionWindow = Boolean(
    (window as typeof window & { __VERSION_INIT__?: object }).__VERSION_INIT__,
  );
  const timerInit = (
    window as typeof window & { __TIMER_INIT__?: TimerInit }
  ).__TIMER_INIT__;

  let editor = $state<Editor>();
  let colorMenuOpen = $state(false);
  let titlebarHovered = $state(false);
  let alwaysOnTop = $state(false);
  let collapsed = $state(false);
  let fontSize = $state(16);
  let growAfterExpandBaseline: number | undefined;
  let fontResizeBaseline:
    | { windowHeight: number; contentHeight: number }
    | undefined;
  let fontResizeFrame: number | undefined;
  let fontResizeRevision = 0;
  let linkBusy = $state(false);
  let noteTitle = $state("Empty Note");
  let moveTimer: number | undefined;
  let geometrySettleRevision = 0;
  const unlisteners: Array<() => void> = [];

  const geometryDebounceMs = 150;
  const mouseReleasePollMs = 50;

  async function toggleAlwaysOnTop() {
    await editor?.flushSave();
    const next = !alwaysOnTop;
    await invoke("set_note_always_on_top", { alwaysOnTop: next });
    alwaysOnTop = next;
  }

  async function linkNotesOnThisSide() {
    if (linkBusy) return;
    linkBusy = true;
    try {
      await editor?.flushSave();
      await invoke("link_windows_on_this_side_below_current_window");
    } finally {
      linkBusy = false;
    }
  }

  async function closeNote() {
    await editor?.flushSave();
    await invoke("close_window");
  }

  async function toggleCollapsed() {
    await editor?.flushSave();
    const next = !collapsed;
    if (next) {
      if (fontResizeFrame !== undefined) {
        cancelAnimationFrame(fontResizeFrame);
        fontResizeFrame = undefined;
      }
      fontResizeRevision += 1;
      fontResizeBaseline = undefined;
    }
    await invoke("set_collapsed", { collapsed: next });
    collapsed = next;
    colorMenuOpen = false;
    if (!collapsed) {
      requestAnimationFrame(() => {
        editor?.focus();
        if (growAfterExpandBaseline !== undefined) {
          growAfterExpandBaseline = undefined;
          void editor?.growToFit();
        }
      });
    }
  }

  function toggleColorMenu() {
    colorMenuOpen = !colorMenuOpen;
  }

  async function setColor(color: string) {
    document.body.style.backgroundColor = color;
    colorMenuOpen = false;
    await editor?.flushSave();
  }

  function cancelGeometrySettlement() {
    geometrySettleRevision += 1;
    if (moveTimer !== undefined) window.clearTimeout(moveTimer);
    moveTimer = undefined;
  }

  function scheduleGeometrySettlement(delay: number) {
    cancelGeometrySettlement();
    const revision = geometrySettleRevision;
    moveTimer = window.setTimeout(() => {
      moveTimer = undefined;
      void settleGeometryAfterMouseRelease(revision);
    }, delay);
  }

  async function settleGeometryAfterMouseRelease(revision: number) {
    const settled = await invoke<boolean>("save_geometry");
    if (!settled && revision === geometrySettleRevision) {
      moveTimer = window.setTimeout(() => {
        moveTimer = undefined;
        void settleGeometryAfterMouseRelease(revision);
      }, mouseReleasePollMs);
    }
  }

  function saveGeometryDebounced() {
    scheduleGeometrySettlement(geometryDebounceMs);
  }

  async function startWindowDrag() {
    cancelGeometrySettlement();
    try {
      await invoke("start_window_drag");
    } finally {
      saveGeometryDebounced();
    }
  }

  function createNoteWithControlN(event: KeyboardEvent) {
    if (
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !event.shiftKey &&
      event.key.toLowerCase() === "n"
    ) {
      event.preventDefault();
      event.stopPropagation();
      void invoke("create_note");
    }
  }

  function changeFontSizeWithShift(event: KeyboardEvent) {
    if (
      event.metaKey &&
      event.shiftKey &&
      !event.ctrlKey &&
      !event.altKey &&
      (event.code === "Equal" || event.code === "Minus")
    ) {
      event.preventDefault();
      event.stopPropagation();
      void invoke("change_font_size", { increase: event.code === "Equal" });
    }
  }

  onMount(async () => {
    if (shortcutsWindow || versionWindow || timerInit) return;
    const init = (window as typeof window & { __STICKY_INIT__?: StickyInit })
      .__STICKY_INIT__;
    alwaysOnTop = init?.always_on_top ?? false;
    collapsed = init?.collapsed ?? false;
    fontSize = init?.font_size ?? 16;

    if (!init) document.body.classList.add("focused");
    window.addEventListener("keydown", createNoteWithControlN, true);
    window.addEventListener("keydown", changeFontSizeWithShift, true);

    unlisteners.push(
      await appWindow.listen("tauri://focus", async () => {
        await invoke("bring_all_to_front");
        titlebarHovered = true;
        document.body.classList.add("focused");
      }),
      await appWindow.listen("tauri://blur", async () => {
        titlebarHovered = false;
        document.body.classList.remove("focused");
        editor?.removeSelection();
        await editor?.flushSave();
      }),
      await appWindow.listen<number>("set_color", async (event) => {
        await setColor(colors[event.payload]);
      }),
      await appWindow.listen<number>("set_font_size", (event) => {
        const previousFontSize = fontSize;
        const increased = event.payload > fontSize;
        const changed = event.payload !== fontSize;
        if (!collapsed && changed && fontResizeBaseline === undefined) {
          const windowHeight = editor?.currentWindowHeight();
          const contentHeight = editor?.currentContentHeight();
          if (windowHeight !== undefined && contentHeight !== undefined) {
            fontResizeBaseline = {
              windowHeight,
              contentHeight,
            };
          }
        }
        fontSize = event.payload;
        if (collapsed) {
          if (increased) {
            growAfterExpandBaseline ??= previousFontSize;
          } else if (
            growAfterExpandBaseline !== undefined &&
            fontSize <= growAfterExpandBaseline
          ) {
            growAfterExpandBaseline = undefined;
          }
        } else if (fontResizeBaseline !== undefined) {
          if (fontResizeFrame !== undefined) {
            cancelAnimationFrame(fontResizeFrame);
          }
          const baseline = fontResizeBaseline;
          fontResizeRevision += 1;
          const revision = fontResizeRevision;
          fontResizeFrame = requestAnimationFrame(() => {
            fontResizeFrame = undefined;
            void (async () => {
              await editor?.resizeForFontSize(
                baseline.windowHeight,
                baseline.contentHeight,
              );
              if (
                fontResizeBaseline === baseline &&
                fontResizeRevision === revision
              ) {
                fontResizeBaseline = undefined;
              }
            })();
          });
        }
      }),
      await appWindow.listen("close_note_request", () => closeNote()),
      await appWindow.listen("tauri://move", saveGeometryDebounced),
      await appWindow.listen("tauri://resize", saveGeometryDebounced),
    );
  });

  onDestroy(() => {
    if (moveTimer !== undefined) window.clearTimeout(moveTimer);
    if (fontResizeFrame !== undefined) cancelAnimationFrame(fontResizeFrame);
    window.removeEventListener("keydown", createNoteWithControlN, true);
    window.removeEventListener("keydown", changeFontSizeWithShift, true);
    unlisteners.forEach((unlisten) => unlisten());
  });
</script>

{#if shortcutsWindow}
  <ShortcutsHelp />
{:else if versionWindow}
  <Version />
{:else if timerInit}
  <Timer init={timerInit} />
{:else}
  <div class="titlebar" class:hover={titlebarHovered} class:collapsed>
  <div
    class="drag-surface"
    onmousedown={(event) => {
      if (event.button === 0 && event.detail === 1) {
        event.preventDefault();
        event.stopImmediatePropagation();
        void startWindowDrag();
      }
    }}
    ondblclick={toggleCollapsed}
    onkeydown={(event) => {
      if (event.key === "Enter" || event.key === " ") void toggleCollapsed();
    }}
    role="button"
    tabindex="0"
    aria-label="Double-click to fold or unfold note"
  >
    <span class="note-title">{noteTitle}</span>
  </div>
  <div class="controls">
    <button
      class="titlebar-button"
      onclick={(event) => {
        event.stopPropagation();
        void closeNote();
      }}
      aria-label="close note"
    >
      <Icon path={mdiClose} size={15} />
    </button>
    <button
      class="titlebar-button"
      onclick={(event) => {
        event.stopPropagation();
        void toggleAlwaysOnTop();
      }}
      aria-label={alwaysOnTop ? "unpin note" : "pin note"}
      aria-pressed={alwaysOnTop}
      title={alwaysOnTop
        ? "Pinned above other apps and across workspaces"
        : "Pin above other apps and across workspaces"}
    >
      <Icon path={alwaysOnTop ? mdiPin : mdiPinOff} size={10} />
    </button>
    <button
      class="titlebar-button"
      disabled={linkBusy}
      onclick={(event) => {
        event.stopPropagation();
        void linkNotesOnThisSide();
      }}
      aria-label="Make this note the parent and relink all windows on this side below it."
      title="Make this the parent and relink all windows on this side below it."
    >
      <Icon path={mdiLinkVariant} size={12} />
    </button>
    <button
      class="titlebar-button"
      onclick={(event) => {
        event.stopPropagation();
        toggleColorMenu();
      }}
      aria-label="select note color"
    >
      <Icon path={mdiPalette} size={10} />
    </button>
    {#if colorMenuOpen}
      {#each colors as color}
        <button
          class="color"
          onclick={(event) => {
            event.stopPropagation();
            void setColor(color);
          }}
          aria-label={`set note color ${color}`}
          style:background={color}
        ></button>
      {/each}
    {/if}
  </div>
  </div>

  <main class:collapsed>
    <Editor
      bind:this={editor}
      {fontSize}
      onTitleChange={(title) => (noteTitle = title)}
    />
  </main>
{/if}

<style>
  .titlebar {
    align-items: center;
    display: flex;
    height: 24px;
    background: rgba(0, 0, 0, 0.07);
    box-sizing: border-box;
    position: relative;
    user-select: none;
    z-index: 2;
  }

  .titlebar.hover {
    background: rgba(0, 0, 0, 0.11);
  }

  .drag-surface {
    inset: 0;
    outline: none;
    position: absolute;
  }

  .note-title {
    font: 600 12px/24px system-ui, sans-serif;
    left: 9px;
    overflow: hidden;
    pointer-events: none;
    position: absolute;
    right: 88px;
    text-align: left;
    text-overflow: ellipsis;
    top: 0;
    white-space: nowrap;
  }

  .titlebar:not(.collapsed) .note-title {
    opacity: 0;
  }

  .controls {
    display: flex;
    flex-direction: row-reverse;
    height: 24px;
    margin-left: auto;
    opacity: 0.42;
    position: relative;
    transition: opacity 120ms ease;
    z-index: 1;
  }

  .titlebar:hover .controls,
  .titlebar.collapsed .controls {
    opacity: 0.9;
  }

  button {
    background-color: transparent;
    border: 0;
    height: 24px;
    margin: 0;
    padding: 0;
    width: 20px;
  }

  .color {
    border: 1px solid rgba(0, 0, 0, 0.24);
    border-radius: 50%;
    height: 14px;
    margin: 5px 2px;
    width: 14px;
  }

  main {
    height: calc(100vh - 24px);
  }

  main.collapsed {
    display: none;
  }
</style>
