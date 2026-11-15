<script lang="ts" module>
  import type { SideView } from "./preferences.svelte";

  /** The rail's views, in order, with their names and shortcuts (Ctrl+Shift
   * and the letter; none of them is a key terminals or tmux rely on). */
  export const SIDE_VIEWS: { view: SideView; name: string; key: string }[] = [
    { view: "hosts", name: "Hosts", key: "H" },
    { view: "snippets", name: "Snippets", key: "S" },
    { view: "jobs", name: "Jobs", key: "J" },
    { view: "backups", name: "Config backups", key: "G" },
    { view: "credentials", name: "Credentials", key: "L" },
  ];
</script>

<script lang="ts">
  // The thin rail of icons on the window's left edge: it picks the side
  // view, and clicking the open view's icon folds the side view away.
  import Archive from "@lucide/svelte/icons/archive";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import ListChecks from "@lucide/svelte/icons/list-checks";
  import Server from "@lucide/svelte/icons/server";
  import Settings from "@lucide/svelte/icons/settings";
  import SquareCode from "@lucide/svelte/icons/square-code";
  import { runningCount } from "./jobs.svelte";

  interface Props {
    /** The view the side view shows (or would show, when folded). */
    view: SideView;
    folded: boolean;
    /** Workspace files could not be loaded. */
    problems: number;
    /** Credential profiles the workspace names but are not set up here. */
    missingProfiles: number;
    onselect: (view: SideView) => void;
    onpreferences: () => void;
  }

  let {
    view,
    folded,
    problems,
    missingProfiles,
    onselect,
    onpreferences,
  }: Props = $props();

  const icons = {
    hosts: Server,
    snippets: SquareCode,
    jobs: ListChecks,
    backups: Archive,
    credentials: KeyRound,
  };

  const running = $derived(runningCount());
  const elsewhere = $derived(runningCount(true));

  /** The icon's tooltip: its name, what a click does, its shortcut, and
   * what its mark says. */
  function title(each: (typeof SIDE_VIEWS)[number]): string {
    const open = each.view === view && !folded;
    let text = `${each.name} (Ctrl+Shift+${each.key})${open ? ": click to fold the side view" : ""}`;
    if (each.view === "hosts" && problems > 0) {
      text += ` · ${problems === 1 ? "1 file" : `${problems} files`} could not be loaded`;
    }
    if (each.view === "jobs" && running > 0) {
      text += ` · ${running} running`;
    }
    if (each.view === "jobs" && elsewhere > 0) {
      text += ` (${elsewhere} more in other workspaces)`;
    }
    if (each.view === "credentials" && missingProfiles > 0) {
      text += ` · ${missingProfiles} not set up here`;
    }
    return text;
  }

  /** Whether the icon carries a mark, and its kind. */
  function mark(each: SideView): "warning" | "running" | null {
    if (each === "hosts" && problems > 0 && (folded || view !== "hosts")) {
      return "warning";
    }
    if (each === "jobs" && running > 0) return "running";
    if (each === "credentials" && missingProfiles > 0) return "warning";
    return null;
  }
</script>

<div
  class="rail"
  role="toolbar"
  aria-label="Side views"
  aria-orientation="vertical"
  data-rail-folded={folded || undefined}
>
  {#each SIDE_VIEWS as each (each.view)}
    {@const Icon = icons[each.view]}
    {@const marked = mark(each.view)}
    <button
      class="icon"
      class:active={each.view === view}
      class:open={each.view === view && !folded}
      data-rail={each.view}
      data-active={each.view === view && !folded ? "" : undefined}
      data-problems={each.view === "hosts" && marked ? problems : undefined}
      data-jobs-running={each.view === "jobs" ? running : undefined}
      aria-pressed={each.view === view && !folded}
      aria-label={each.name}
      title={title(each)}
      onclick={() => onselect(each.view)}
    >
      <Icon size={20} aria-hidden="true" />
      {#if marked}
        <span class="mark {marked}" aria-hidden="true"></span>
      {/if}
    </button>
  {/each}
  <button
    class="icon foot"
    data-open-preferences
    aria-label="Preferences"
    title="Preferences: theme, terminal font, scrollback, copy and paste (Ctrl+,)"
    onclick={onpreferences}
  >
    <Settings size={20} aria-hidden="true" />
  </button>
</div>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-2) 0;
    background: var(--bg);
    border-right: 1px solid var(--border);
    min-height: 0;
  }

  .icon {
    position: relative;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-md);
    background: none;
    color: var(--muted);
  }

  .icon:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-raised);
  }

  /* The view the side view is on: brighter, and a bar on the rail's edge
     while it is open. */
  .icon.active {
    color: var(--text);
  }

  .icon.open {
    background: var(--selected);
  }

  .icon.open::before {
    content: "";
    position: absolute;
    left: calc(-1 * var(--space-1));
    top: 6px;
    bottom: 6px;
    width: 2px;
    border-radius: var(--radius-full);
    background: var(--accent);
  }

  .foot {
    margin-top: auto;
  }

  .mark {
    position: absolute;
    top: 5px;
    right: 5px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    border: 1.5px solid var(--bg);
    box-sizing: content-box;
  }

  .mark.warning {
    background: var(--warning);
  }

  .mark.running {
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }

  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .mark.running {
      animation: none;
    }
  }
</style>
