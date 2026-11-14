<script lang="ts">
  import {
    changePreferences,
    FONT_SIZES,
    MAX_SCROLLBACK,
    preferences,
    SUGGESTED_FONTS,
    type Theme,
  } from "./preferences.svelte";
  import { highlighting } from "./highlight.svelte";
  import { checkNow, describeStatus, updates } from "./updates.svelte";
  import VaultSection from "./VaultSection.svelte";
  import PythonSection from "./PythonSection.svelte";
  import { python } from "./python.svelte";

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  const current = $derived(preferences.current);

  /** A whole number from a number field, or null when it holds none. */
  function wholeNumber(input: HTMLInputElement): number | null {
    const value = Math.round(input.valueAsNumber);
    return Number.isFinite(value) ? value : null;
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    // A field being typed in is saved (its change event), not dropped.
    if (document.activeElement instanceof HTMLElement)
      document.activeElement.blur();
    onclose();
  }

  const SHORTCUTS: [string, string][] = [
    ["Copy", "Ctrl+Shift+C, Ctrl+Insert, or Ctrl+C with text selected"],
    ["Paste", "Ctrl+Shift+V, Ctrl+V or Shift+Insert"],
    ["Search the scrollback", "Ctrl+Shift+F; then Enter / Shift+Enter"],
    ["Font size", "Ctrl+scroll, Ctrl+= / Ctrl+- (Ctrl+0 resets)"],
  ];
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="preferences-title"
  data-dialog="preferences"
>
  <section>
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
          onclick={() => {
            onclose();
            highlighting.dialog = true;
          }}>Rules…</button
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
            : ""}.
        {/if}
        {updates.busy ? "Checking…" : describeStatus(updates.status)}
      </p>
    </div>

    {#if preferences.error}
      <p class="error" role="alert">{preferences.error}</p>
    {/if}

    <VaultSection />

    <PythonSection
      onsetup={() => {
        onclose();
        python.dialog = true;
      }}
    />

    <dl>
      {#each SHORTCUTS as [what, keys] (what)}
        <dt>{what}</dt>
        <dd>{keys}</dd>
      {/each}
    </dl>

    <div class="actions">
      <!-- svelte-ignore a11y_autofocus -->
      <button autofocus onclick={onclose}>Close</button>
    </div>
  </section>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 15;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: rgb(0 0 0 / 0.45);
  }

  section {
    width: min(30rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: 0.9rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.4);
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.5rem 0.8rem;
    font-size: 0.85rem;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .status {
    margin: 0;
    color: var(--muted);
    font-size: 0.8rem;
    overflow-wrap: anywhere;
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }

  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 0.25rem 0.8rem;
    margin: 0;
    padding-top: 0.6rem;
    border-top: 1px solid var(--border);
    font-size: 0.8rem;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
