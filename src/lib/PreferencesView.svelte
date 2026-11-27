<script lang="ts">
  import {
    changePreferences,
    FONT_SIZES,
    MAX_SCROLLBACK,
    preferences,
    reservedKeys,
    isDark,
    SUGGESTED_FONTS,
    terminalTheme,
    type ColorScheme,
    type Theme,
  } from "./preferences.svelte";
  import {
    COLOR_NAMES,
    COLOR_SCHEMES,
    themeKey,
    type ColorName,
  } from "./colorSchemes";
  import {
    actionFor,
    chordFor,
    chordOf,
    SHORTCUT_ACTIONS,
  } from "./keyBindings";
  import { inventory } from "./inventory.svelte";
  import { localShells, type LocalShell } from "./session";
  import { openView } from "./views";
  import { checkNow, describeStatus, updates } from "./updates.svelte";
  import VaultSection from "./VaultSection.svelte";
  import PythonSection from "./PythonSection.svelte";

  const current = $derived(preferences.current);

  /** The shells this computer has, for the local shell choice (#22). */
  let shells = $state<LocalShell[]>([]);
  localShells().then(
    (list) => (shells = list),
    () => (shells = []),
  );

  /** A whole number from a number field, or null when it holds none. */
  function wholeNumber(input: HTMLInputElement): number | null {
    const value = Math.round(input.valueAsNumber);
    return Number.isFinite(value) ? value : null;
  }

  /** The colour a name has now: the user's own, else the scheme's. */
  function colorNow(name: ColorName): string {
    const value = terminalTheme(isDark())[themeKey(name)];
    return typeof value === "string" ? value : "#000000";
  }

  function setColor(name: ColorName, color: string) {
    void changePreferences({
      customColors: { ...current.customColors, [name]: color },
    });
  }

  function resetColor(name: ColorName) {
    const { [name]: _gone, ...rest } = current.customColors;
    void changePreferences({ customColors: rest });
  }

  const snippetNames = $derived(
    (inventory.workspace?.snippets ?? []).map((snippet) => snippet.name),
  );

  /** A key pressed in a binding field: the chord replaces the field's, or
   * (Escape) nothing changes. Tab and the like leave the field as usual. */
  function captureChord(event: KeyboardEvent): string | null {
    if (event.key === "Tab" && !event.ctrlKey && !event.altKey) return null;
    event.preventDefault();
    event.stopPropagation();
    return chordOf(event);
  }

  /** Why a chord cannot be used: what the app does with it, or null. */
  function reservedReason(chord: string): string | null {
    const found = reservedKeys.list.find((each) => each.chord === chord);
    return found ? `${chord} is kept by the app: it ${found.what}.` : null;
  }

  /** The refusal of the last key pressed, shown beside the fields. */
  let refused = $state<string | null>(null);

  /** A key to record: null (nothing changes, the reason shown) when the app
   * keeps it. */
  function allowed(chord: string | null): string | null {
    refused = chord === null ? null : reservedReason(chord);
    return refused === null ? chord : null;
  }

  /** What a chord is bound to now, for a warning on a clash. */
  function boundTo(chord: string, except: string): string | null {
    const action = actionFor(chord, current.shortcuts);
    if (action !== null && action !== except) {
      return SHORTCUT_ACTIONS.find((each) => each.id === action)!.name;
    }
    return current.snippetKeys[chord] !== undefined &&
      current.snippetKeys[chord] !== except
      ? `the snippet ${current.snippetKeys[chord]}`
      : null;
  }

  function moveShortcut(id: string, chord: string) {
    const default_ = SHORTCUT_ACTIONS.find((each) => each.id === id)!.chord;
    const { [id]: _old, ...rest } = current.shortcuts;
    void changePreferences({
      shortcuts: chord === default_ ? rest : { ...rest, [id]: chord },
    });
  }

  function bindSnippet(chord: string, name: string) {
    void changePreferences({
      snippetKeys: { ...current.snippetKeys, [chord]: name },
    });
  }

  function unbindSnippet(chord: string) {
    const { [chord]: _gone, ...rest } = current.snippetKeys;
    void changePreferences({ snippetKeys: rest });
  }

  let newChord = $state("");
  let newSnippet = $state("");

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

      <label for="preference-color-scheme">Colour scheme</label>
      <select
        id="preference-color-scheme"
        name="preference-color-scheme"
        value={current.colorScheme}
        title="The terminal's colours; highlight rules that name a colour follow them"
        onchange={(event) =>
          changePreferences({
            colorScheme: event.currentTarget.value as ColorScheme,
          })}
      >
        {#each COLOR_SCHEMES as scheme (scheme.id)}
          <option value={scheme.id}>{scheme.name}</option>
        {/each}
      </select>

      <span></span>
      <details class="colors" data-own-colors>
        <summary>
          Own colours{Object.keys(current.customColors).length > 0
            ? ` (${Object.keys(current.customColors).length} changed)`
            : ""}
        </summary>
        <div class="swatches">
          {#each COLOR_NAMES as name (name)}
            <label class="swatch">
              <input
                type="color"
                name="preference-color-{name}"
                value={colorNow(name)}
                onchange={(event) => setColor(name, event.currentTarget.value)}
              />
              {name}
              {#if current.customColors[name]}
                <button
                  type="button"
                  class="reset"
                  title="Back to the scheme's colour"
                  onclick={(event) => {
                    event.preventDefault();
                    resetColor(name);
                  }}>Reset</button
                >
              {/if}
            </label>
          {/each}
        </div>
      </details>

      <span></span>
      <details class="colors" data-key-bindings>
        <summary>
          Key bindings{Object.keys(current.shortcuts).length +
            Object.keys(current.snippetKeys).length >
          0
            ? " (changed)"
            : ""}
        </summary>
        <p class="hint">
          Click a field and press the keys. Ctrl or Alt is needed (a function
          key stands alone). A key bound to a snippet is no longer sent to the
          device. Keys the app keeps (copy and paste, search, the tab and font
          size keys, the side views) cannot be used. Where the key that prints a
          letter and the key at its place name different actions, the printed
          letter wins.
        </p>
        {#if refused}
          <p class="clash" role="status" data-key-refused>{refused}</p>
        {/if}
        <div class="bindings">
          {#each SHORTCUT_ACTIONS as action (action.id)}
            {@const chord = chordFor(action.id, current.shortcuts)}
            <label for="preference-key-{action.id}">{action.name}</label>
            <span class="binding">
              <input
                id="preference-key-{action.id}"
                name="preference-key-{action.id}"
                data-chord-capture
                readonly
                value={chord}
                onkeydown={(event) => {
                  const next = allowed(captureChord(event));
                  if (next !== null) moveShortcut(action.id, next);
                }}
              />
              {#if current.shortcuts[action.id]}
                <button
                  type="button"
                  onclick={() => moveShortcut(action.id, action.chord)}
                  >Reset</button
                >
              {/if}
              {#if boundTo(chord, action.id)}
                <span class="clash" role="status"
                  >Also {boundTo(chord, action.id)}</span
                >
              {/if}
            </span>
          {/each}
        </div>
        <h3>Snippet keys</h3>
        <div class="bindings">
          {#each Object.entries(current.snippetKeys) as [chord, name] (chord)}
            <span class="chord">{chord}</span>
            <span class="binding">
              {name}
              {#if !snippetNames.includes(name)}
                <span class="clash">no such snippet</span>
              {/if}
              <button type="button" onclick={() => unbindSnippet(chord)}
                >Remove</button
              >
            </span>
          {/each}
          <input
            name="preference-new-snippet-key"
            data-chord-capture
            readonly
            placeholder="Press keys"
            value={newChord}
            onkeydown={(event) => {
              const next = allowed(captureChord(event));
              if (next !== null) newChord = next;
            }}
          />
          <span class="binding">
            <select
              name="preference-new-snippet-key-snippet"
              bind:value={newSnippet}
            >
              <option value="">Snippet…</option>
              {#each snippetNames as name (name)}
                <option value={name}>{name}</option>
              {/each}
            </select>
            <button
              type="button"
              disabled={newChord === "" || newSnippet === ""}
              onclick={() => {
                bindSnippet(newChord, newSnippet);
                newChord = "";
                newSnippet = "";
              }}>Add</button
            >
          </span>
        </div>
      </details>

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

      <label for="preference-local-shell">Local shell</label>
      <select
        id="preference-local-shell"
        name="preference-local-shell"
        value={current.localShell}
        title="The shell a new local shell tab opens"
        onchange={(event) =>
          changePreferences({ localShell: event.currentTarget.value })}
      >
        <option value="">Default</option>
        {#each shells as shell (shell.id)}
          <option value={shell.id}>{shell.name}</option>
        {/each}
      </select>

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
  .colors summary {
    cursor: pointer;
    color: var(--muted);
  }

  .swatches {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr));
    gap: var(--space-2);
    padding-top: var(--space-2);
  }

  .swatch {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
  }

  .bindings {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: var(--space-2);
    align-items: center;
    padding-top: var(--space-2);
    font-size: var(--text-sm);
  }

  .binding {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }

  .hint,
  .clash {
    color: var(--muted);
    font-size: var(--text-sm);
  }

  h3 {
    margin: var(--space-3) 0 0;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  .swatch .reset {
    margin-left: auto;
  }

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
