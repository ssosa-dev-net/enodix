<script lang="ts">
  // A detached window (#77): one run, outside the main window, on any
  // monitor. No rail, no terminals: the run view, live from the shell's
  // record of jobs, with Dock back. It follows the theme and font
  // preferences; closing it leaves the job running.
  import { listen } from "@tauri-apps/api/event";
  import { invoke } from "@tauri-apps/api/core";
  import RunView from "./lib/RunView.svelte";
  import { startLiveJobs } from "./lib/jobs.svelte";
  import {
    inventory,
    setWorkspace,
    workspaceChanged,
  } from "./lib/inventory.svelte";
  import {
    fontStack,
    isDark,
    loadPreferences,
    preferences,
    type Preferences,
  } from "./lib/preferences.svelte";
  import { onWorkspaceChanged, type WorkspaceView } from "./lib/workspace";
  import { appName } from "./lib/session";
  import { dockBack } from "./lib/windows";

  interface Props {
    /** The run it shows. */
    run: string;
  }

  let { run }: Props = $props();

  let loaded = $state(false);
  let error = $state<string | null>(null);

  void loadPreferences();
  // Changed in the main window: shown here too.
  void listen<Preferences>("preferences-changed", (event) => {
    preferences.current = event.payload;
  });

  $effect(() => {
    document.documentElement.dataset.theme = isDark() ? "dark" : "light";
    document.documentElement.style.setProperty(
      "--font-mono",
      fontStack(preferences.current.fontFamily),
    );
  });

  void onWorkspaceChanged(workspaceChanged);
  invoke<WorkspaceView | null>("workspace_current").then(
    (view) => {
      setWorkspace(view);
      loaded = true;
    },
    (reason) => (error = String(reason)),
  );
  void startLiveJobs();

  appName().then((name) => (document.title = `${name} - run ${run}`));
</script>

<main class="detached" data-detached="run" data-detached-run={run}>
  {#if error}
    <p class="note" role="alert">{error}</p>
  {:else if !loaded}
    <p class="note">Loading…</p>
  {:else if !inventory.workspace}
    <p class="note" data-detached-no-workspace>
      No workspace is open in the main window.
      <button type="button" data-run-action="dock" onclick={() => dockBack()}
        >Dock back</button
      >
    </p>
  {:else}
    <RunView {run} detached onclose={() => dockBack()} />
  {/if}
</main>

<style>
  .detached {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--text);
    overflow: hidden;
  }
  .detached > :global(*) {
    flex: 1;
    min-height: 0;
  }
  .note {
    flex: none;
    margin: var(--space-4);
    color: var(--muted);
  }
</style>
