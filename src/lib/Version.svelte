<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  interface UpdateStatus {
    installed_sha: string;
    latest_sha: string;
    update_available: boolean;
  }

  type CheckState = "checking" | "up-to-date" | "update-available" | "check-failed";

  const initialSha = (
    window as typeof window & {
      __VERSION_INIT__?: { installed_sha?: string | null };
    }
  ).__VERSION_INIT__?.installed_sha;

  let checkState = $state<CheckState>("checking");
  let installedSha = $state(initialSha ?? "");
  let latestSha = $state("");
  let checkError = $state("");
  let installError = $state("");
  let installing = $state(false);
  let installationStarted = $state(false);

  function shortSha(sha: string) {
    return sha.slice(0, 7);
  }

  function errorMessage(error: unknown) {
    return typeof error === "string" ? error : "The update check failed unexpectedly. Try again.";
  }

  async function check() {
    checkState = "checking";
    checkError = "";
    installError = "";
    installationStarted = false;
    try {
      const result = await invoke<UpdateStatus>("check_for_update");
      installedSha = result.installed_sha;
      latestSha = result.latest_sha;
      checkState = result.update_available ? "update-available" : "up-to-date";
    } catch (error) {
      checkError = errorMessage(error);
      checkState = "check-failed";
    }
  }

  async function update() {
    if (installing || installationStarted) return;
    installing = true;
    installError = "";
    try {
      await invoke("launch_update");
      installationStarted = true;
    } catch (error) {
      installError =
        typeof error === "string" ? error : "Sticky could not start the update in Terminal.";
    } finally {
      installing = false;
    }
  }

  onMount(() => void check());
</script>

<main>
  <div class="app-mark" aria-hidden="true">S</div>
  <h1>Sticky</h1>
  <p class="build">{installedSha ? `Build ${shortSha(installedSha)}` : "Unknown build"}</p>

  <section aria-live="polite">
    {#if installationStarted}
      <h2>Installation started</h2>
      <p>No input is needed in Terminal. Keep it open; Sticky will quit and reopen when the update finishes.</p>
    {:else if checkState === "checking"}
      <h2>Checking for updates…</h2>
      <p>Comparing this build with GitHub main.</p>
    {:else if checkState === "up-to-date"}
      <h2>Up to date</h2>
      <p>This build matches GitHub main.</p>
    {:else if checkState === "update-available"}
      <h2>Update available</h2>
      <p>Latest build <span class="commit">{shortSha(latestSha)}</span> is available.</p>
      {#if installError}<p class="error">{installError}</p>{/if}
      <button disabled={installing} onclick={() => void update()}>
        {installing ? "Starting…" : "Update"}
      </button>
    {:else}
      <h2>Check failed</h2>
      <p class="error">{checkError}</p>
      <button onclick={() => void check()}>Retry</button>
    {/if}
  </section>
</main>

<style>
  :global(body) {
    background: #f4f0dc;
    color: #211f18;
  }

  main {
    box-sizing: border-box;
    height: 100%;
    padding: 24px 28px;
    text-align: center;
  }

  .app-mark {
    align-items: center;
    background: #fff3a0;
    border: 1px solid rgba(33, 31, 24, 0.12);
    border-radius: 13px;
    box-shadow: 0 5px 14px rgba(33, 31, 24, 0.1);
    display: flex;
    font: 700 25px/1 system-ui, sans-serif;
    height: 50px;
    justify-content: center;
    margin: 0 auto 10px;
    transform: rotate(-2deg);
    width: 50px;
  }

  h1 {
    font-size: 22px;
    letter-spacing: -0.02em;
    margin: 0;
  }

  .build {
    color: rgba(33, 31, 24, 0.6);
    font: 500 12px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
    margin: 3px 0 18px;
  }

  section {
    border-top: 1px solid rgba(33, 31, 24, 0.1);
    padding-top: 15px;
  }

  h2 {
    font-size: 14px;
    margin: 0 0 5px;
  }

  section p {
    color: rgba(33, 31, 24, 0.67);
    font-size: 12px;
    line-height: 1.45;
    margin: 0 auto 12px;
    max-width: 330px;
  }

  .commit {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  section p.error {
    color: #8a2d21;
  }

  button {
    background: #38362e;
    border: 0;
    border-radius: 7px;
    color: white;
    font: 600 12px system-ui, sans-serif;
    min-width: 82px;
    padding: 8px 14px;
  }

  button:hover:not(:disabled) {
    background: #211f18;
  }

  button:disabled {
    opacity: 0.55;
  }
</style>
