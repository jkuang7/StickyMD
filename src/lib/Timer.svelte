<script lang="ts">
  import { mdiClose, mdiCog, mdiLinkVariant, mdiPin, mdiPinOff } from "@mdi/js";
  import { invoke } from "@tauri-apps/api/core";
  import { webviewWindow } from "@tauri-apps/api";
  import { onDestroy, onMount, untrack } from "svelte";

  import Icon from "$lib/Icon.svelte";

  interface TimerSnapshot {
    id: string;
    elapsed_ms: number;
    running: boolean;
    reminder_interval_ms: number;
    alarm_at_ms: number;
    always_on_top: boolean;
    collapsed: boolean;
  }

  interface TimerTick {
    elapsed_ms: number;
    running: boolean;
  }

  let { init }: { init: TimerSnapshot } = $props();

  const appWindow = webviewWindow.getCurrentWebviewWindow();
  const initial = untrack(() => init);
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
  let elapsedMs = $state(initial.elapsed_ms);
  let dateLabel = $state(dateFormatter.format(new Date()));
  let running = $state(initial.running);
  let alwaysOnTop = $state(initial.always_on_top);
  let collapsed = $state(initial.collapsed);
  let reminderIntervalMs = $state(initial.reminder_interval_ms);
  let alarmAtMs = $state(initial.alarm_at_ms);
  let settingsOpen = $state(false);
  let titlebarHovered = $state(false);
  let busy = $state(false);
  let errorMessage = $state("");
  let elapsedEditing = $state(false);
  let elapsedHours = $state(
    Math.floor(initial.elapsed_ms / 3_600_000).toString().padStart(2, "0"),
  );
  let elapsedMinutes = $state(
    (Math.floor(initial.elapsed_ms / 60_000) % 60)
      .toString()
      .padStart(2, "0"),
  );
  let elapsedSeconds = $state(
    (Math.floor(initial.elapsed_ms / 1_000) % 60)
      .toString()
      .padStart(2, "0"),
  );
  let reminderHours = $state(
    Math.floor(initial.reminder_interval_ms / 3_600_000),
  );
  let reminderMinutes = $state(
    Math.floor(initial.reminder_interval_ms / 60_000) % 60,
  );
  let reminderSeconds = $state(
    Math.floor(initial.reminder_interval_ms / 1_000) % 60,
  );
  let alarmHours = $state(Math.floor(initial.alarm_at_ms / 3_600_000));
  let alarmMinutes = $state(
    Math.floor(initial.alarm_at_ms / 60_000) % 60,
  );
  let alarmSeconds = $state(
    Math.floor(initial.alarm_at_ms / 1_000) % 60,
  );
  const unlisteners: Array<() => void> = [];
  let elapsedSave = Promise.resolve();
  let elapsedSavePending = 0;
  let elapsedSaveRevision = 0;
  let requestedElapsedKey = elapsedKey(initial.elapsed_ms);

  const formattedElapsed = $derived(formatElapsed(elapsedMs));
  const displayFontSize = $derived(
    Math.max(
      25,
      48 -
        Math.max(
          0,
          (running ? formattedElapsed.length : elapsedHours.length + 6) - 8,
        ) *
          4,
    ),
  );
  const actionLabel = $derived(
    running ? "Pause" : elapsedMs === 0 ? "Start" : "Resume",
  );

  function formatElapsed(milliseconds: number) {
    const seconds = Math.floor(milliseconds / 1_000);
    const hours = Math.floor(seconds / 3_600)
      .toString()
      .padStart(2, "0");
    const minutes = Math.floor(seconds / 60) % 60;
    const remainingSeconds = seconds % 60;
    return `${hours}:${minutes.toString().padStart(2, "0")}:${remainingSeconds
      .toString()
      .padStart(2, "0")}`;
  }

  function acceptSnapshot(snapshot: TimerSnapshot) {
    elapsedMs = snapshot.elapsed_ms;
    running = snapshot.running;
    reminderIntervalMs = snapshot.reminder_interval_ms;
    alarmAtMs = snapshot.alarm_at_ms;
    alwaysOnTop = snapshot.always_on_top;
    if (!elapsedEditing) syncElapsedFields();
  }

  function syncElapsedFields() {
    elapsedHours = Math.floor(elapsedMs / 3_600_000)
      .toString()
      .padStart(2, "0");
    elapsedMinutes = (Math.floor(elapsedMs / 60_000) % 60)
      .toString()
      .padStart(2, "0");
    elapsedSeconds = (Math.floor(elapsedMs / 1_000) % 60)
      .toString()
      .padStart(2, "0");
    requestedElapsedKey = elapsedKey(elapsedMs);
  }

  function elapsedKey(milliseconds: number) {
    const totalSeconds = Math.floor(milliseconds / 1_000);
    return `${Math.floor(totalSeconds / 3_600)}:${Math.floor(totalSeconds / 60) % 60}:${totalSeconds % 60}`;
  }

  function parseElapsedField(value: string, maximum?: number) {
    if (!/^\d+$/.test(value)) return undefined;
    const parsed = Number(value);
    if (
      !Number.isSafeInteger(parsed) ||
      (maximum !== undefined && parsed > maximum)
    ) {
      return undefined;
    }
    return parsed;
  }

  function selectElapsedField(event: Event) {
    (event.currentTarget as HTMLInputElement).select();
  }

  function beginElapsedEdit(event: FocusEvent) {
    elapsedEditing = true;
    selectElapsedField(event);
  }

  function beginElapsedPointerEdit(event: MouseEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    const input = event.currentTarget as HTMLInputElement;
    input.focus();
    input.select();
  }

  function commitElapsedEdit() {
    elapsedEditing = false;
    const hours = parseElapsedField(elapsedHours);
    const minutes = parseElapsedField(elapsedMinutes, 59);
    const seconds = parseElapsedField(elapsedSeconds, 59);
    if (hours === undefined || minutes === undefined || seconds === undefined) {
      errorMessage = "Use whole values; minutes and seconds must be 0–59.";
      syncElapsedFields();
      return;
    }

    const requestKey = `${hours}:${minutes}:${seconds}`;
    if (requestKey === requestedElapsedKey) {
      syncElapsedFields();
      return;
    }

    errorMessage = "";
    requestedElapsedKey = requestKey;
    elapsedSaveRevision += 1;
    const revision = elapsedSaveRevision;
    elapsedSavePending += 1;
    elapsedSave = elapsedSave.then(async () => {
      let snapshot: TimerSnapshot | undefined;
      try {
        snapshot = await invoke<TimerSnapshot>("timer_set_elapsed", {
          hours,
          minutes,
          seconds,
        });
      } catch (error) {
        if (revision === elapsedSaveRevision) errorMessage = String(error);
      } finally {
        elapsedSavePending -= 1;
      }
      if (snapshot && revision === elapsedSaveRevision) {
        acceptSnapshot(snapshot);
      } else if (!snapshot && revision === elapsedSaveRevision && !elapsedEditing) {
        syncElapsedFields();
      }
    });
  }

  function finishElapsedEditOnEnter(event: KeyboardEvent) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    (event.currentTarget as HTMLInputElement).blur();
  }

  function refreshSoundFields() {
    reminderHours = Math.floor(reminderIntervalMs / 3_600_000);
    reminderMinutes = Math.floor(reminderIntervalMs / 60_000) % 60;
    reminderSeconds = Math.floor(reminderIntervalMs / 1_000) % 60;
    alarmHours = Math.floor(alarmAtMs / 3_600_000);
    alarmMinutes = Math.floor(alarmAtMs / 60_000) % 60;
    alarmSeconds = Math.floor(alarmAtMs / 1_000) % 60;
  }

  function validWholeNumber(value: number, maximum?: number) {
    return (
      Number.isSafeInteger(value) &&
      value >= 0 &&
      (maximum === undefined || value <= maximum)
    );
  }

  async function runAction() {
    if (busy) return;
    await elapsedSave;
    busy = true;
    errorMessage = "";
    try {
      acceptSnapshot(
        await invoke<TimerSnapshot>(running ? "timer_pause" : "timer_resume"),
      );
    } catch (error) {
      errorMessage = String(error);
    } finally {
      busy = false;
    }
  }

  async function resetTimer() {
    if (busy) return;
    await elapsedSave;
    busy = true;
    errorMessage = "";
    try {
      acceptSnapshot(await invoke<TimerSnapshot>("timer_reset"));
    } catch (error) {
      errorMessage = String(error);
    } finally {
      busy = false;
    }
  }

  async function applySettings() {
    errorMessage = "";
    if (
      !validWholeNumber(reminderHours) ||
      !validWholeNumber(reminderMinutes, 59) ||
      !validWholeNumber(reminderSeconds, 59) ||
      !validWholeNumber(alarmHours) ||
      !validWholeNumber(alarmMinutes, 59) ||
      !validWholeNumber(alarmSeconds, 59)
    ) {
      errorMessage = "Use whole values; minutes and seconds must be 0–59.";
      return;
    }

    busy = true;
    try {
      acceptSnapshot(
        await invoke<TimerSnapshot>("timer_apply_settings", {
          reminderHours,
          reminderMinutes,
          reminderSeconds,
          alarmHours,
          alarmMinutes,
          alarmSeconds,
        }),
      );
      refreshSoundFields();
      settingsOpen = false;
    } catch (error) {
      errorMessage = String(error);
    } finally {
      busy = false;
    }
  }

  async function toggleAlwaysOnTop() {
    const next = !alwaysOnTop;
    errorMessage = "";
    try {
      await invoke("set_timer_always_on_top", { alwaysOnTop: next });
      alwaysOnTop = next;
    } catch (error) {
      errorMessage = String(error);
    }
  }

  async function closeTimer() {
    errorMessage = "";
    try {
      await invoke("close_window");
    } catch (error) {
      errorMessage = String(error);
    }
  }

  async function startWindowDrag() {
    try {
      await invoke("start_window_drag");
      await invoke("save_geometry");
    } catch (error) {
      errorMessage = String(error);
    }
  }

  async function toggleCollapsed() {
    if (busy) return;
    const next = !collapsed;
    errorMessage = "";
    settingsOpen = false;
    try {
      await invoke("set_collapsed", { collapsed: next });
      collapsed = next;
    } catch (error) {
      errorMessage = String(error);
    }
  }

  async function linkWindowsOnThisSide() {
    if (busy) return;
    if (!window.confirm("Are you sure you want to link these windows?")) return;
    busy = true;
    errorMessage = "";
    try {
      await invoke("link_windows_on_this_side_below_current_window");
    } catch (error) {
      errorMessage = String(error);
    } finally {
      busy = false;
    }
  }

  async function toggleSettings() {
    if (!settingsOpen && collapsed) {
      try {
        await invoke("set_collapsed", { collapsed: false });
        collapsed = false;
      } catch (error) {
        errorMessage = String(error);
        return;
      }
    }
    if (!settingsOpen) refreshSoundFields();
    errorMessage = "";
    settingsOpen = !settingsOpen;
  }

  onMount(async () => {
    document.body.classList.add("timer-window");
    unlisteners.push(
      await appWindow.listen<TimerTick>("timer_tick", (event) => {
        elapsedMs = event.payload.elapsed_ms;
        running = event.payload.running;
        dateLabel = dateFormatter.format(new Date());
        if (!elapsedEditing && elapsedSavePending === 0) syncElapsedFields();
      }),
      await appWindow.listen("tauri://focus", async () => {
        await invoke("bring_all_to_front");
        titlebarHovered = true;
        document.body.classList.add("focused");
      }),
      await appWindow.listen("tauri://blur", () => {
        titlebarHovered = false;
        document.body.classList.remove("focused");
      }),
      await appWindow.listen("tauri://move", () => {
        void invoke("save_geometry");
      }),
    );
  });

  onDestroy(() => {
    document.body.classList.remove("timer-window");
    unlisteners.forEach((unlisten) => unlisten());
  });
</script>

<div class="timer-titlebar" class:hover={titlebarHovered} class:collapsed>
  <div
    class="timer-drag-surface"
    onmousedown={(event) => {
      if (event.button === 0 && event.detail === 1) {
        event.preventDefault();
        event.stopImmediatePropagation();
        void startWindowDrag();
      }
    }}
    ondblclick={() => void toggleCollapsed()}
    onkeydown={(event) => {
      if (event.key === "Enter" || event.key === " ") void toggleCollapsed();
    }}
    role="button"
    tabindex="0"
    aria-label="Double-click to fold or unfold timer"
  >
    <span>Timer</span>
  </div>
  <div class="timer-controls">
    <button
      class="titlebar-button"
      onclick={(event) => {
        event.stopPropagation();
        void closeTimer();
      }}
      aria-label="close timer"
    >
      <Icon path={mdiClose} size={15} />
    </button>
    <button
      class="titlebar-button"
      disabled={busy}
      onclick={(event) => {
        event.stopPropagation();
        void linkWindowsOnThisSide();
      }}
      aria-label="Make this timer the parent and relink all windows on this side below it."
      title="Make this the parent and relink all windows on this side below it."
    >
      <Icon path={mdiLinkVariant} size={12} />
    </button>
    <button
      class="titlebar-button"
      onclick={(event) => {
        event.stopPropagation();
        void toggleAlwaysOnTop();
      }}
      aria-label={alwaysOnTop ? "unpin timer" : "pin timer"}
      aria-pressed={alwaysOnTop}
      title={alwaysOnTop
        ? "Pinned above other apps and across workspaces"
        : "Pin above other apps and across workspaces"}
    >
      <Icon path={alwaysOnTop ? mdiPin : mdiPinOff} size={10} />
    </button>
    <button
      class="titlebar-button"
      onclick={(event) => {
        event.stopPropagation();
        void toggleSettings();
      }}
      aria-label="timer settings"
      aria-expanded={settingsOpen}
    >
      <Icon path={mdiCog} size={12} />
    </button>
  </div>
</div>

{#if !collapsed}
<main class="timer-body">
  <div class="timer-date">{dateLabel}</div>
  {#if running}
    <div
      class="timer-display"
      style:font-size={`${displayFontSize}px`}
      aria-label={`${formattedElapsed} elapsed`}
    >
      {formattedElapsed}
    </div>
  {:else}
    <div
      class="timer-display editable"
      style:font-size={`${displayFontSize}px`}
      aria-label="Editable elapsed time"
    >
      <input
        class="hours"
        aria-label="Elapsed hours"
        inputmode="numeric"
        autocomplete="off"
        disabled={settingsOpen}
        style:width={`${Math.max(2, elapsedHours.length)}ch`}
        bind:value={elapsedHours}
        onmousedown={beginElapsedPointerEdit}
        onfocus={beginElapsedEdit}
        onclick={selectElapsedField}
        onblur={commitElapsedEdit}
        onkeydown={finishElapsedEditOnEnter}
      />
      <span>:</span>
      <input
        aria-label="Elapsed minutes"
        inputmode="numeric"
        autocomplete="off"
        maxlength="2"
        disabled={settingsOpen}
        bind:value={elapsedMinutes}
        onmousedown={beginElapsedPointerEdit}
        onfocus={beginElapsedEdit}
        onclick={selectElapsedField}
        onblur={commitElapsedEdit}
        onkeydown={finishElapsedEditOnEnter}
      />
      <span>:</span>
      <input
        aria-label="Elapsed seconds"
        inputmode="numeric"
        autocomplete="off"
        maxlength="2"
        disabled={settingsOpen}
        bind:value={elapsedSeconds}
        onmousedown={beginElapsedPointerEdit}
        onfocus={beginElapsedEdit}
        onclick={selectElapsedField}
        onblur={commitElapsedEdit}
        onkeydown={finishElapsedEditOnEnter}
      />
    </div>
  {/if}
  <div class="timer-actions">
    <button disabled={busy} onclick={() => void resetTimer()}>Reset</button>
    <button class="primary" disabled={busy} onclick={() => void runAction()}>
      {actionLabel}
    </button>
  </div>
  {#if errorMessage && !settingsOpen}
    <p class="timer-error">{errorMessage}</p>
  {/if}
</main>

{#if settingsOpen}
  <section class="settings-popover" aria-label="Timer settings">
    <div class="settings-row">
      <span>Beep every</span>
      <label>
        <input
          type="number"
          min="0"
          step="1"
          disabled={busy}
          bind:value={reminderHours}
        />h
      </label>
      <label>
        <input
          type="number"
          min="0"
          max="59"
          step="1"
          disabled={busy}
          bind:value={reminderMinutes}
        />m
      </label>
      <label>
        <input
          type="number"
          min="0"
          max="59"
          step="1"
          disabled={busy}
          bind:value={reminderSeconds}
        />s
      </label>
    </div>
    <div class="settings-row">
      <span>Alarm at</span>
      <label>
        <input
          type="number"
          min="0"
          step="1"
          disabled={busy}
          bind:value={alarmHours}
        />h
      </label>
      <label>
        <input
          type="number"
          min="0"
          max="59"
          step="1"
          disabled={busy}
          bind:value={alarmMinutes}
        />m
      </label>
      <label>
        <input
          type="number"
          min="0"
          max="59"
          step="1"
          disabled={busy}
          bind:value={alarmSeconds}
        />s
      </label>
    </div>
    <div class="settings-footer">
      <span class:error-visible={Boolean(errorMessage)}>{errorMessage}</span>
      <button disabled={busy} onclick={() => void applySettings()}>Apply</button>
    </div>
  </section>
{/if}
{/if}

<style>
  :global(body.timer-window) {
    background: #d6d4bd;
    color: #22251e;
  }

  .timer-titlebar {
    align-items: center;
    background: rgba(0, 0, 0, 0.08);
    box-sizing: border-box;
    display: flex;
    height: 24px;
    position: relative;
    user-select: none;
    z-index: 4;
  }

  .timer-titlebar.hover {
    background: rgba(0, 0, 0, 0.12);
  }

  .timer-drag-surface {
    inset: 0;
    position: absolute;
  }

  .timer-drag-surface span {
    font: 600 12px/24px system-ui, sans-serif;
    left: 9px;
    pointer-events: none;
    position: absolute;
    top: 0;
  }

  .timer-controls {
    display: flex;
    flex-direction: row-reverse;
    height: 24px;
    margin-left: auto;
    opacity: 0.55;
    position: relative;
    transition: opacity 120ms ease;
    z-index: 1;
  }

  .timer-titlebar:hover .timer-controls {
    opacity: 0.95;
  }

  .titlebar-button {
    background: transparent;
    border: 0;
    height: 24px;
    margin: 0;
    padding: 0;
    width: 22px;
  }

  .timer-body {
    align-items: center;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    height: 152px;
    padding: 8px 16px 12px;
  }

  .timer-date {
    color: rgba(34, 37, 30, 0.72);
    font: 600 11px/14px system-ui, sans-serif;
    margin-bottom: 8px;
  }

  .timer-display {
    color: #26371f;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-variant-numeric: tabular-nums;
    line-height: 1;
    max-width: 100%;
    white-space: nowrap;
  }

  .timer-display.editable {
    align-items: baseline;
    display: flex;
  }

  .timer-display.editable input {
    -webkit-appearance: none;
    appearance: none;
    background: rgba(255, 255, 255, 0.2);
    border: 0;
    border-radius: 2px;
    box-shadow: none;
    box-sizing: content-box;
    color: inherit;
    font: inherit;
    height: 1em;
    line-height: 1;
    margin: 0;
    min-width: 2ch;
    outline: none;
    padding: 0 1px;
    text-align: center;
    width: 2ch;
  }

  .timer-display.editable input:focus {
    background: rgba(255, 255, 255, 0.48);
  }

  .timer-display.editable input:disabled {
    opacity: 1;
  }

  .timer-display.editable span {
    line-height: 1;
  }

  .timer-actions {
    display: flex;
    gap: 8px;
    margin-top: 11px;
  }

  .timer-actions button,
  .settings-footer button {
    background: rgba(255, 255, 255, 0.34);
    border: 1px solid rgba(24, 29, 20, 0.22);
    border-radius: 6px;
    font: 600 12px system-ui, sans-serif;
    min-width: 64px;
    padding: 6px 12px;
  }

  .timer-actions button.primary,
  .settings-footer button {
    background: #455e38;
    border-color: #455e38;
    color: white;
  }

  button:disabled {
    opacity: 0.5;
  }

  .timer-error {
    color: #8a2525;
    font: 10px/1.2 system-ui, sans-serif;
    margin: 7px 0 0;
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .settings-popover {
    background: #e4e2ce;
    bottom: 0;
    box-sizing: border-box;
    left: 0;
    padding: 12px 13px 10px;
    position: absolute;
    right: 0;
    top: 24px;
    z-index: 3;
  }

  .settings-row {
    align-items: center;
    display: grid;
    font: 12px system-ui, sans-serif;
    grid-template-columns: 1fr repeat(3, 55px);
    margin-bottom: 10px;
  }

  .settings-row > span {
    font-weight: 600;
  }

  .settings-row label {
    align-items: center;
    display: flex;
    gap: 3px;
  }

  .settings-row input {
    background: rgba(255, 255, 255, 0.66);
    border: 1px solid rgba(24, 29, 20, 0.22);
    border-radius: 4px;
    box-sizing: border-box;
    font: 12px ui-monospace, SFMono-Regular, Menlo, monospace;
    height: 25px;
    padding: 2px 4px;
    width: 43px;
  }

  .settings-footer {
    align-items: center;
    display: flex;
    justify-content: flex-end;
  }

  .settings-footer span {
    color: #8a2525;
    display: none;
    font: 10px/1.15 system-ui, sans-serif;
    margin-right: auto;
    max-width: 185px;
  }

  .settings-footer span.error-visible {
    display: block;
  }
</style>
