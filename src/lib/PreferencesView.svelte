<script lang="ts">
  import {
    changePreferences,
    FONT_SIZES,
    MAX_SCROLLBACK,
    preferences,
    SUGGESTED_FONTS,
    type Theme,
  } from "./preferences.svelte";
  import { openView } from "./views";
  import { checkNow, describeStatus, updates } from "./updates.svelte";
  import VaultSection from "./VaultSection.svelte";
  import PythonSection from "./PythonSection.svelte";

  const current = $derived(preferences.current);

  /** A whole number from a number field, or null when it holds none. */
  function wholeNumber(input: HTMLInputElement): number | null {
    const value = Math.round(input.valueAsNumber);
    return Number.isFinite(value) ? value : null;
  }

  const SHORTCUTS: [string, string][] = [
    ["Copy", "Ctrl+Shift+C, Ctrl+Insert, or Ctrl+C with text selected"],
    ["Paste", "Ctrl+Shift+V, Ctrl+V or Shift+Insert"],
    ["Search the scrollback", "Ctrl+Shift+F; then Enter / Shift+Enter"],
    ["Font size", "Ctrl+scroll, Ctrl+= / Ctrl+- (Ctrl+0 resets)"],
  ];
</script>

<!-- A view tab (#68): beside the terminals, so a change shows at once on
     a terminal in the pane next to it. Every field saves on change. -->
<div class="view" data-view="preferences">
  <section aria-labelledby="preferences-title">
    <h2 id="preferences-title">Preferences</h2>
    <div class="fields">
      <label for="preference-theme">Theme</label>
      <select
        id="preference-theme"
        name="preference-theme"
        value={current.theme}
        onchange={(event) =>
          changePreferences({ theme: event.currentTarget.value as Theme })}
      >
        <option value="system">Same as the system</option>
        <option value="light">Light</option>
        <option value="dark">Dark</option>
      </select>

      <label for="preference-font-family">Terminal font</label>
      <input
        id="preference-font-family"
        name="preference-font-family"
        list="preference-fonts"
        value={current.fontFamily}
        onchange={(event) =>
          changePreferences({ fontFamily: event.currentTarget.value })}
      />
      <datalist id="preference-fonts">
        {#each SUGGESTED_FONTS as font (font)}
          <option value={font}></option>
        {/each}
      </datalist>

      <label for="preference-font-size">Font size</label>
      <input
        id="preference-font-size"
        name="preference-font-size"
        type="number"
        min={FONT_SIZES.min}
        max={FONT_SIZES.max}
        value={current.fontSize}
        onchange={(event) => {
          const size = wholeNumber(event.currentTarget);
          if (size !== null) void changePreferences({ fontSize: size });
          else event.currentTarget.value = String(current.fontSize);
        }}
      />

      <label for="preference-scrollback">Scrollback lines</label>
      <input
        id="preference-scrollback"
        name="preference-scrollback"
        type="number"
        min="0"
        max={MAX_SCROLLBACK}
        step="1000"
        value={current.scrollback}
        onchange={(event) => {
          const lines = wholeNumber(event.currentTarget);
          if (lines !== null)
            void changePreferences({ scrollback: Math.max(0, lines) });
          else event.currentTarget.value = String(current.scrollback);
        }}
      />

      <span></span>
      <label class="check">
        <input
          type="checkbox"
          name="preference-copy-on-select"
          checked={current.copyOnSelect}
          onchange={(event) =>
            changePreferences({ copyOnSelect: event.currentTarget.checked })}
        />
        Copy text when it is selected
      </label>

      <span></span>
      <label class="check">
        <input
          type="checkbox"
          name="preference-right-click-paste"
          checked={current.rightClickPaste}
          onchange={(event) =>
            changePreferences({
              rightClickPaste: event.currentTarget.checked,
            })}
        />
        Paste with a right click in the terminal
      </label>

      <span></span>
      <label
        class="check"
        title="Production hosts always ask, whatever this is set to"
      >
        <input
          type="checkbox"
          name="preference-confirm-multi-line-paste"
          checked={current.confirmMultiLinePaste}
          onchange={(event) =>
            changePreferences({
              confirmMultiLinePaste: event.currentTarget.checked,
            })}
        />
        Ask before pasting more than one line (production hosts always ask)
      </label>

      <span></span>
      <span class="check">
        <label class="check">
          <input
            type="checkbox"
            name="preference-highlighting"
            checked={current.highlighting}
            onchange={(event) =>
              changePreferences({ highlighting: event.currentTarget.checked })}
          />
          Highlight keywords in output
        </label>
        <button
          type="button"
          data-open-highlights
          onclick={() => openView("highlights")}>Rules…</button
        >
      </span>

      <span></span>
      <span class="check updates">
        <label
          class="check"
          title="Off, the app connects nowhere unless you ask it to"
        >
          <input
            type="checkbox"
            name="preference-check-for-updates"
            checked={current.checkForUpdates}
            onchange={(event) =>
              changePreferences({
                checkForUpdates: event.currentTarget.checked,
              })}
          />
          Check for a new version at start
        </label>
        <button
          type="button"
          data-update-check-now
          disabled={updates.busy || updates.build?.configured === false}
          onclick={() => void checkNow()}>Check now</button
        >
      </span>
      <span></span>
      <p class="status" data-update-status={updates.status?.kind}>
        {#if updates.build}
          Version {updates.build.version}{updates.build.kind === "portable"
            ? " (portable: new versions are announced, never installed)"
            : updates.build.kind === "package"
              ? " (installed by a package: new versions are announced and installed with the system's package manager)"
              : ""}.
        {/if}
        {updates.busy ? "Checking…" : describeStatus(updates.status)}
      </p>
    </div>

    {#if preferences.error}
      <p class="error" role="alert">{preferences.error}</p>
    {/if}

    <VaultSection />

    <PythonSection />

    <dl>
      {#each SHORTCUTS as [what, keys] (what)}
        <dt>{what}</dt>
        <dd>{keys}</dd>
      {/each}
    </dl>
  </section>
</div>

<style>
  .view {
    height: 100%;
    overflow: auto;
    background: var(--bg);
  }

  section {
    width: min(36rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-5) var(--space-4);
    margin: 0 auto;
  }

  h2 {
    margin: 0;
    font-size: var(--text-xl);
    font-weight: var(--weight-semibold);
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2) var(--space-3);
    font-size: var(--text-sm);
  }

  .fields > label:not(.check) {
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--muted);
  }

  .check {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
  }

  .status {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }

  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: var(--space-1) var(--space-3);
    margin: 0;
    padding-top: var(--space-2);
    border-top: 1px solid var(--border);
    font-size: var(--text-xs);
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
  }
</style>
