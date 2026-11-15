<script lang="ts">
  // A detached window (#77, #78): one run or one host's config history,
  // outside the main window, on any monitor. No rail, no terminals: the
  // run view (live from the shell's record of jobs) or the config history
  // (refreshed on every backup), with Dock back. It follows the theme and
  // font preferences; closing it leaves a job running.
  import { listen } from "@tauri-apps/api/event";
  import { invoke } from "@tauri-apps/api/core";
  import RunView from "./lib/RunView.svelte";
  import ConfigHistory from "./lib/ConfigHistory.svelte";
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
  import { dockBack, type DetachedView } from "./lib/windows";
  import { findHost } from "./lib/workspace";

  interface Props {
    /** What it shows. */
    view: DetachedView;
  }

  let { view }: Props = $props();

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

  let name = $state("");
  appName().then((value) => (name = value));
  $effect(() => {
    if (!name) return;
    document.title =
      view.kind === "run"
        ? `${name} - run ${view.run}`
        : `${name} - config history ${hostName(view.host)}`;
  });

  function hostName(id: string): string {
    const tree = inventory.workspace?.tree;
    return (tree && findHost(tree, id)?.name) ?? id;
  }
</script>

<main
  class="detached"
  data-detached={view.kind}
  data-detached-run={view.kind === "run" ? view.run : undefined}
  data-detached-host={view.kind === "configs" ? view.host : undefined}
>
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
  {:else if view.kind === "run"}
    <RunView run={view.run} detached onclose={() => dockBack()} />
  {:else}
    <ConfigHistory host={view.host} detached />
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
