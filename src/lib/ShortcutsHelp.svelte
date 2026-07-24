<script lang="ts">
  import { webviewWindow } from "@tauri-apps/api";
  import { onMount } from "svelte";

  const appWindow = webviewWindow.getCurrentWebviewWindow();
  const sections = [
    {
      title: "Notes",
      shortcuts: [
        ["New note", "⌘ N"],
        ["New timer", "⇧ ⌘ N"],
        ["Close window", "⌘ W"],
        ["Reopen last closed note", "⇧ ⌘ T"],
        ["Restore all notes", "⇧ ⌘ U"],
        ["Reset window positions", "⇧ ⌘ R"],
        ["Hide or show all notes", "⇧ ⌘ H"],
        ["Quit Sticky", "⌘ Q"],
      ],
    },
    {
      title: "Editor",
      shortcuts: [
        ["Increase font size", "⌘ +"],
        ["Decrease font size", "⌘ -"],
        ["Bullet list", "⇧ ⌘ 0"],
        ["Checklist", "⇧ ⌘ 9"],
        ["Check or uncheck task", "⇧ ⌘ C"],
        ["Delete completed tasks", "⇧ ⌘ X"],
        ["Strikethrough", "⇧ ⌘ S"],
        ["Indent list item", "Tab"],
        ["Outdent list item", "⇧ Tab"],
      ],
    },
    {
      title: "Navigate & link",
      shortcuts: [
        ["Relink all windows on this side below current window", "⇧ ⌘ L"],
        ["Align with next note or screen edge", "⌥ ⌘ + Arrow"],
        ["Align with any nearby edge", "⇧ ⌥ ⌘ + Arrow"],
        ["Set color 1–7", "⌘ 1–7"],
      ],
    },
  ];

  function closeOnEscape(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.preventDefault();
    void appWindow.close();
  }

  onMount(() => {
    window.addEventListener("keydown", closeOnEscape, true);
    return () => window.removeEventListener("keydown", closeOnEscape, true);
  });
</script>

<main>
  <header>
    <div>
      <h1>Keyboard Shortcuts</h1>
      <p>Press <kbd>⌘ /</kbd> again or <kbd>Esc</kbd> to close.</p>
    </div>
    <button onclick={() => void appWindow.close()} aria-label="close keyboard shortcuts">
      Done
    </button>
  </header>

  {#each sections as section}
    <section>
      <h2>{section.title}</h2>
      <dl>
        {#each section.shortcuts as shortcut}
          <div>
            <dt>{shortcut[0]}</dt>
            <dd><kbd>{shortcut[1]}</kbd></dd>
          </div>
        {/each}
      </dl>
    </section>
  {/each}
</main>

<style>
  :global(body) {
    background: #f4f0dc;
    color: #211f18;
    overflow: hidden;
  }

  main {
    box-sizing: border-box;
    height: 100%;
    overflow-y: auto;
    padding: 22px 24px 28px;
  }

  header {
    align-items: flex-start;
    display: flex;
    justify-content: space-between;
    margin-bottom: 18px;
  }

  h1 {
    font-size: 24px;
    letter-spacing: -0.02em;
    margin: 0 0 4px;
  }

  header p {
    color: rgba(33, 31, 24, 0.62);
    font-size: 12px;
    margin: 0;
  }

  button {
    background: rgba(33, 31, 24, 0.09);
    border: 0;
    border-radius: 6px;
    font: 600 12px system-ui, sans-serif;
    padding: 7px 11px;
  }

  button:hover {
    background: rgba(33, 31, 24, 0.15);
  }

  section + section {
    margin-top: 17px;
  }

  h2 {
    color: rgba(33, 31, 24, 0.6);
    font-size: 11px;
    letter-spacing: 0.08em;
    margin: 0 0 6px;
    text-transform: uppercase;
  }

  dl {
    background: rgba(255, 255, 255, 0.42);
    border: 1px solid rgba(33, 31, 24, 0.1);
    border-radius: 9px;
    margin: 0;
    overflow: hidden;
  }

  dl div {
    align-items: center;
    display: flex;
    justify-content: space-between;
    min-height: 29px;
    padding: 0 10px;
  }

  dl div + div {
    border-top: 1px solid rgba(33, 31, 24, 0.08);
  }

  dt {
    font-size: 13px;
  }

  dd {
    margin: 0;
  }

  kbd {
    background: rgba(255, 255, 255, 0.75);
    border: 1px solid rgba(33, 31, 24, 0.14);
    border-bottom-color: rgba(33, 31, 24, 0.25);
    border-radius: 5px;
    box-shadow: 0 1px 0 rgba(33, 31, 24, 0.11);
    display: inline-block;
    font: 600 11px system-ui, sans-serif;
    min-width: 17px;
    padding: 2px 6px;
    text-align: center;
  }
</style>
