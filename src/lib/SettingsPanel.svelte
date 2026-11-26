<script lang="ts">
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import Info from "@lucide/svelte/icons/info";
  import X from "@lucide/svelte/icons/x";
  import Button from "./Button.svelte";
  import IconButton from "./IconButton.svelte";
  import {
    describeOrigin,
    inventory,
    isOwn,
    platformDefinition,
    type Item,
  } from "./inventory.svelte";
  import { tick } from "svelte";
  import { profileNames } from "./profiles.svelte";
  import { openView } from "./views";
  import { serialPorts, type SerialPortInfo } from "./session";
  import {
    MAX_PASTE_LINE_DELAY_MS,
    type Effective,
    type EffectiveSettings,
    type Folder,
    type Origin,
    type Settings,
  } from "./workspace";

  interface Props {
    item: Item;
    /** What the host or folder sets itself. */
    settings: Settings;
    /** The settings as saved, when `settings` holds unsaved edits (the
     * host editor, #10): an edited setting says so instead of what applied
     * before the edit. */
    saved?: Settings;
    /** What applies to it (null while loading). */
    effective: EffectiveSettings | null;
    /** Host ids a jump host can be chosen from. */
    hostIds: string[];
    onchange: (change: (current: Settings) => Settings) => Promise<void>;
    /**
     * Which settings: the device's (platform), or the connection's (with
     * the serial line and web links). The details sheet puts each in its
     * own section, under its own heading.
     */
    part: "device" | "connection";
  }

  let { item, settings, saved, effective, hostIds, onchange, part }: Props =
    $props();

  const uid = $props.id();
  /** Which groups of inherited settings are shown, by group. */
  let inheritedShown = $state<Record<string, boolean>>({});
  /** Whose help note is open, by setting. */
  let noteShown = $state<Record<string, boolean>>({});
  /** The serial line was asked for, though nothing says it is used. */
  let serialAsked = $state(false);

  /** Settings whose value is being overridden (the input is shown), by key. */
  let overriding = $state<Record<string, boolean>>({});
  /** Why a setting's value was refused, by key; shown under its field. */
  let errors = $state<Record<string, string>>({});

  type Options = readonly (readonly [string, string])[];

  interface Row {
    key: string;
    label: string;
    input:
      | { kind: "choice"; options: Options }
      | { kind: "number"; min: number; max: number; list?: string }
      | { kind: "text"; list?: string };
    /** What the item sets, as the control shows it; "" when unset. */
    own: (settings: Settings) => string;
    /** The settings with this one changed; "" clears it. */
    set: (settings: Settings, value: string) => Settings;
    effective: (settings: EffectiveSettings) => Effective<unknown> | null;
    /** What applies when nothing sets it and there is no default. */
    unset?: string;
    /** What the user should know about the setting, shown under it. */
    note?: string;
  }

  const ON_OFF: Options = [
    ["on", "On"],
    ["off", "Off"],
  ];
  const flag = (value: boolean | null) =>
    value === null ? "" : value ? "on" : "off";
  const toFlag = (value: string) => (value === "" ? null : value === "on");
  const toNumber = (value: string) =>
    value.trim() === "" ? null : Number(value);
  const toText = (value: string) => (value.trim() === "" ? null : value);
  const show = (value: unknown) =>
    typeof value === "boolean" ? (value ? "on" : "off") : String(value);

  const device: Row[] = [
    {
      key: "platform",
      label: "Platform",
      input: { kind: "text", list: "platforms" },
      own: (s) => s.platform ?? "",
      set: (s, v) => ({ ...s, platform: toText(v) }),
      effective: (e) => e.platform,
      unset: "none",
    },
  ];

  /**
   * Offered for the platform setting: the usable definitions, then every
   * other platform the workspace names that finds none.
   */
  const platformNames = $derived.by(() => {
    const defined = (inventory.workspace?.platforms ?? [])
      .filter((listed) => listed.error === null)
      .map((listed) => listed.definition.name);
    const names = new Set<string>();
    const walk = (folder: Folder) => {
      if (folder.settings.platform) names.add(folder.settings.platform);
      for (const host of folder.hosts) {
        if (host.settings.platform) names.add(host.settings.platform);
      }
      folder.folders.forEach(walk);
    };
    if (inventory.workspace) walk(inventory.workspace.tree);
    const others = [...names]
      .filter((name) => platformDefinition(name) === null)
      .sort((a, b) => a.localeCompare(b));
    return [...defined, ...others];
  });

  /** The definition the platform that applies finds, if one applies. */
  const platformMatch = $derived.by(() => {
    const platform = effective?.platform?.value;
    if (!platform) return null;
    const found = platformDefinition(platform);
    return {
      platform,
      definition: found?.usable ? found.definition : null,
      broken: found && !found.usable ? found.definition : null,
    };
  });

  const connection: Row[] = [
    {
      key: "protocol",
      label: "Default protocol",
      input: {
        kind: "choice",
        options: [
          ["ssh", "SSH"],
          ["telnet", "Telnet"],
          ["serial", "Serial"],
        ],
      },
      own: (s) => s.protocol ?? "",
      set: (s, v) => ({ ...s, protocol: (v || null) as Settings["protocol"] }),
      effective: (e) => e.protocol,
    },
    {
      key: "port",
      label: "Port",
      input: { kind: "number", min: 1, max: 65535 },
      own: (s) => s.port?.toString() ?? "",
      set: (s, v) => ({ ...s, port: toNumber(v) }),
      effective: (e) => e.port,
      unset: "the protocol's default",
    },
    {
      key: "credential_profile",
      label: "Credential profile",
      input: { kind: "text", list: "credential-profiles" },
      own: (s) => s.credential_profile ?? "",
      set: (s, v) => ({ ...s, credential_profile: toText(v) }),
      effective: (e) => e.credential_profile,
      unset: "none",
      note:
        "The login to use: the name of a profile in the Credentials view, or " +
        "a new name, set up the first time you connect. Without one, the " +
        "login is asked at each connection.",
    },
    {
      key: "jump_host",
      label: "Jump host",
      input: { kind: "text", list: "jump-hosts" },
      own: (s) => s.jump_host ?? "",
      set: (s, v) => ({ ...s, jump_host: toText(v) }),
      effective: (e) => e.jump_host,
      unset: "none (direct)",
      note:
        "Another host of the workspace, named by its id (folder and file, " +
        "as the list offers).",
    },
    {
      key: "legacy_crypto",
      label: "Legacy crypto",
      input: { kind: "choice", options: ON_OFF },
      own: (s) => flag(s.legacy_crypto),
      set: (s, v) => ({ ...s, legacy_crypto: toFlag(v) }),
      effective: (e) => e.legacy_crypto,
    },
    {
      key: "connect_timeout",
      label: "Connect timeout (seconds)",
      input: { kind: "number", min: 1, max: 3600 },
      own: (s) => s.connect_timeout?.toString() ?? "",
      set: (s, v) => ({ ...s, connect_timeout: toNumber(v) }),
      effective: (e) => e.connect_timeout,
    },
    {
      key: "logging",
      label: "Logging",
      input: { kind: "choice", options: ON_OFF },
      own: (s) => flag(s.logging),
      set: (s, v) => ({ ...s, logging: toFlag(v) }),
      effective: (e) => e.logging,
      note:
        "Every session is saved as a text file on this computer, outside the workspace. " +
        "Logs can contain secrets shown on screen, such as passwords, keys or " +
        "configurations a device prints. Turn logging off for sensitive devices.",
    },
    {
      key: "encoding",
      label: "Character encoding",
      input: {
        kind: "choice",
        options: [
          ["utf-8", "UTF-8"],
          ["iso-8859-1", "ISO 8859-1 (Latin-1)"],
          ["windows-1252", "Windows-1252"],
          ["cp437", "CP437 (DOS line drawing)"],
        ],
      },
      own: (s) => s.encoding ?? "",
      set: (s, v) => ({
        ...s,
        encoding: (v || null) as Settings["encoding"],
      }),
      effective: (e) => e.encoding,
      note:
        "How the device's bytes stand for characters. The terminal and the session " +
        "log always show UTF-8; what the device sends is converted, and what you " +
        "type is converted to the device's encoding (a character it lacks becomes " +
        "?). Applies to the next connection; jobs and backups still read UTF-8.",
    },
    {
      key: "auto_reconnect",
      label: "Reconnect automatically",
      input: { kind: "choice", options: ON_OFF },
      own: (s) => flag(s.auto_reconnect),
      set: (s, v) => ({ ...s, auto_reconnect: toFlag(v) }),
      effective: (e) => e.auto_reconnect,
      note:
        "A session that drops tries again by itself, waiting longer each time, with " +
        "the count shown; any key stops it. It never repeats a refused login " +
        "(accounts lock) and never answers a host-key question.",
    },
    {
      key: "log_timestamps",
      label: "Log timestamps",
      input: { kind: "choice", options: ON_OFF },
      own: (s) => flag(s.log_timestamps),
      set: (s, v) => ({ ...s, log_timestamps: toFlag(v) }),
      effective: (e) => e.log_timestamps,
      note:
        "Writes the local time before each line of the session log, for change " +
        "records and incident timelines. Has no effect while logging is off.",
    },
    {
      key: "backup_on_login",
      label: "Back up on login",
      input: { kind: "choice", options: ON_OFF },
      own: (s) => flag(s.backup_on_login),
      set: (s, v) => ({ ...s, backup_on_login: toFlag(v) }),
      effective: (e) => e.backup_on_login,
      note:
        "Each SSH session saves the configuration in the host's config history once " +
        "connected, over a second channel on the same connection: nothing shows in " +
        "the terminal. A device that refuses a second channel gets a second login " +
        "with the host's stored credential profile, so one that asks a one-time code " +
        "or sends a push at login does so twice. It does not apply to telnet " +
        "or serial hosts, or hosts without a usable platform definition with a " +
        "show-config command. Backups are as sensitive as the configurations themselves.",
    },
    {
      key: "environment",
      label: "Environment",
      input: {
        kind: "choice",
        options: [
          ["production", "Production"],
          ["staging", "Staging"],
          ["lab", "Lab"],
        ],
      },
      own: (s) => s.environment ?? "",
      set: (s, v) => ({
        ...s,
        environment: (v || null) as Settings["environment"],
      }),
      effective: (e) => e.environment,
      unset: "none",
      note:
        "Production sessions get a red tab and border, and a paste of more " +
        "than one line always asks first.",
    },
    {
      key: "paste_line_delay",
      label: "Paste line delay (ms)",
      input: { kind: "number", min: 0, max: MAX_PASTE_LINE_DELAY_MS },
      own: (s) => s.paste_line_delay?.toString() ?? "",
      set: (s, v) => ({ ...s, paste_line_delay: toNumber(v) }),
      effective: (e) => e.paste_line_delay,
      note: "A wait between pasted lines, for devices that drop fast input.",
    },
    {
      key: "terminal_type",
      label: "Terminal type",
      input: {
        kind: "choice",
        options: [
          ["xterm-256color", "xterm-256color"],
          ["xterm", "xterm"],
          ["vt100", "vt100"],
          ["ansi", "ansi"],
          ["linux", "linux"],
        ],
      },
      own: (s) => s.terminal_type ?? "",
      set: (s, v) => ({
        ...s,
        terminal_type: (v || null) as Settings["terminal_type"],
      }),
      effective: (e) => e.terminal_type,
      note:
        "What the session announces to the device (TERM). Some older devices " +
        "and console servers draw their menus wrongly with xterm-256color. " +
        "Applies to the next connection.",
    },
  ];

  type Serial = Settings["serial"];
  const serialRow = (
    key: keyof Serial,
    label: string,
    input: Row["input"],
    parse: (value: string) => unknown,
  ): Row => ({
    key: `serial.${key}`,
    label,
    input,
    own: (s) => s.serial[key]?.toString() ?? "",
    set: (s, v) => ({ ...s, serial: { ...s.serial, [key]: parse(v) } }),
    effective: (e) => e.serial[key],
    unset: key === "port" ? "none: choose one to open by serial" : undefined,
  });
  const choices = (...values: string[]): Options =>
    values.map((value) => [value, value]);

  const serial: Row[] = [
    serialRow("port", "Port", { kind: "text", list: "serial-ports" }, toText),
    serialRow(
      "baud_rate",
      "Baud rate",
      { kind: "number", min: 1, max: 4294967295, list: "baud-rates" },
      toNumber,
    ),
    serialRow(
      "data_bits",
      "Data bits",
      { kind: "choice", options: choices("5", "6", "7", "8") },
      toNumber,
    ),
    serialRow(
      "parity",
      "Parity",
      {
        kind: "choice",
        // Kept for files that name them; serial ports refuse to open so.
        options: [
          ["none", "none"],
          ["odd", "odd"],
          ["even", "even"],
          ["mark", "mark (not supported by serial ports)"],
          ["space", "space (not supported by serial ports)"],
        ],
      },
      (v) => v || null,
    ),
    serialRow(
      "stop_bits",
      "Stop bits",
      { kind: "choice", options: choices("1", "2") },
      toNumber,
    ),
    serialRow(
      "flow_control",
      "Flow control",
      { kind: "choice", options: choices("none", "hardware", "software") },
      (v) => v || null,
    ),
  ];

  /** The detected serial ports, offered for the port setting. */
  let ports = $state<SerialPortInfo[]>([]);
  let portsError = $state<string | null>(null);
  async function refreshPorts() {
    try {
      ports = await serialPorts();
      portsError = null;
    } catch (error) {
      portsError = `Cannot list the serial ports: ${error}`;
    }
  }
  // Only the connection part offers the serial ports.
  $effect(() => {
    if (part === "connection") void refreshPorts();
  });

  /** The serial line matters when it is the protocol or a port is set. */
  const serialUsed = $derived(
    effective?.protocol.value === "serial" ||
      settings.serial.port != null ||
      effective?.serial.port != null,
  );

  const BAUD_RATES = [
    1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400,
  ];

  /** Why `value` cannot be a setting's value, when that is plain to see. */
  function check(row: Row, value: string): string | null {
    if (row.input.kind !== "number" || value.trim() === "") return null;
    const number = Number(value);
    const { min, max } = row.input;
    if (!Number.isInteger(number))
      return `${row.label} must be a whole number.`;
    if (number < min || number > max) {
      return `${row.label} must be between ${min} and ${max}.`;
    }
    return null;
  }

  /** What an overridden setting starts from: what applies now. */
  function prefill(row: Row): string {
    const applied = effective ? row.effective(effective) : null;
    return applied ? show(applied.value) : "";
  }

  /** The control of `row`. */
  const controlId = (row: Row) => `${uid}-in-${row.key}`;
  const errorId = (row: Row) => `${uid}-err-${row.key}`;

  /**
   * Saves a setting edited in `control`. A value that is refused stays in
   * the control, with the reason under it; one that is saved shows as the
   * file holds it.
   */
  async function commit(
    row: Row,
    value: string,
    control?: HTMLInputElement | HTMLSelectElement,
  ) {
    const problem = check(row, value);
    if (problem) {
      errors[row.key] = problem;
      overriding[row.key] = true;
      return;
    }
    delete errors[row.key];
    const expected = row.own(row.set(settings, value));
    await onchange((current) => row.set(current, value));
    // Without the host editor's draft, a refused save is shown by the
    // inventory above the tree; this field says it instead.
    if (
      saved === undefined &&
      inventory.editError !== null &&
      row.own(settings) !== expected
    ) {
      errors[row.key] = inventory.editError;
      overriding[row.key] = true;
      inventory.editError = null;
      return;
    }
    if (control)
      control.value =
        row.own(settings) || (overriding[row.key] ? prefill(row) : "");
  }

  /** Shows the input of an inherited setting, to give it a value. */
  async function override(row: Row) {
    overriding[row.key] = true;
    await tick();
    document.getElementById(controlId(row))?.focus();
  }

  /** Back to the inherited value. */
  async function useInherited(row: Row) {
    overriding[row.key] = false;
    delete errors[row.key];
    if (row.own(settings) !== "") await commit(row, "");
  }

  /** Where an inherited value comes from, for its chip. */
  function inheritedFrom(origin: Origin): string {
    if (origin.kind === "default") return "Default";
    if (origin.kind === "host") return "this host";
    return `Inherited from ${origin.path === "" ? "workspace" : origin.path}`;
  }

  let newLinkName = $state("");
  let newLinkUrl = $state("");

  function addLink(event: SubmitEvent) {
    event.preventDefault();
    const link = { name: newLinkName, url: newLinkUrl };
    onchange((current) => ({
      ...current,
      web_links: [
        ...current.web_links.filter((l) => l.name !== link.name.trim()),
        link,
      ],
    })).then(() => {
      if (settings.web_links.some((saved) => saved.name === link.name.trim())) {
        newLinkName = "";
        newLinkUrl = "";
      }
    });
  }

  function changeLink(name: string, url: string) {
    return onchange((current) => ({
      ...current,
      web_links: current.web_links.map((link) =>
        link.name === name ? { ...link, url } : link,
      ),
    }));
  }

  function removeLink(name: string) {
    return onchange((current) => ({
      ...current,
      web_links: current.web_links.filter((link) => link.name !== name),
    }));
  }
</script>

{#snippet settingRow(row: Row)}
  {@const own = row.own(settings)}
  {@const applied = effective ? row.effective(effective) : null}
  {@const edited = saved !== undefined && row.own(saved) !== own}
  {@const error = errors[row.key]}
  {@const inherits = own === "" && !overriding[row.key] && !error}
  {@const warning =
    row.key === "logging" &&
    (own === "" ? applied?.value === true : own === "on")}
  {@const describedBy =
    [
      error ? errorId(row) : null,
      row.note && noteShown[row.key] ? `${uid}-note-${row.key}` : null,
    ]
      .filter(Boolean)
      .join(" ") || undefined}
  <div class="setting" data-setting={row.key}>
    <div class="field">
      <!-- The help button sits beside the label, not on a row of its own. -->
      <span class="label">
        <label for={controlId(row)}>{row.label}</label>
        {#if row.note && !warning}
          <IconButton
            icon={Info}
            size="compact"
            label="About {row.label}"
            aria-expanded={noteShown[row.key] === true}
            aria-controls="{uid}-note-{row.key}"
            onclick={() => (noteShown[row.key] = !noteShown[row.key])}
          />
        {/if}
      </span>
      {#if inherits}
        <span class="row chip-row" data-inherited={row.key}>
          <span class="chip" data-edited={edited || undefined}>
            {#if edited}
              <span class="chip-origin">Inherited</span>
            {:else if !effective}
              <span class="chip-value">…</span>
            {:else if applied}
              <span class="chip-value">{show(applied.value)}</span>
              <span class="chip-origin">{inheritedFrom(applied.origin)}</span>
            {:else}
              <span class="chip-value">{row.unset ?? "not set"}</span>
            {/if}
          </span>
          <Button
            size="compact"
            data-override={row.key}
            aria-label="Override {row.label}"
            onclick={() => override(row)}>Override</Button
          >
        </span>
      {/if}
      <!-- Kept in the page when the setting is inherited (hidden). -->
      <span class="row" hidden={inherits}>
        {#if row.input.kind === "choice"}
          <select
            id={controlId(row)}
            name="setting-{row.key}"
            value={own || (overriding[row.key] ? prefill(row) : "")}
            aria-invalid={error ? "true" : undefined}
            aria-describedby={describedBy}
            onchange={(event) =>
              commit(row, event.currentTarget.value, event.currentTarget)}
          >
            <option value="">Inherit</option>
            {#each row.input.options as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        {:else}
          <input
            id={controlId(row)}
            name="setting-{row.key}"
            type={row.input.kind}
            min={row.input.kind === "number" ? row.input.min : undefined}
            max={row.input.kind === "number" ? row.input.max : undefined}
            list={row.input.list}
            spellcheck="false"
            value={own || (overriding[row.key] ? prefill(row) : "")}
            aria-invalid={error ? "true" : undefined}
            aria-describedby={describedBy}
            oninput={() => delete errors[row.key]}
            onkeydown={(event) => {
              // Enter gives an overridden setting the value it shows, even
              // when that is the inherited one (no change to commit).
              if (event.key === "Enter" && own === "") {
                commit(row, event.currentTarget.value, event.currentTarget);
              }
            }}
            onchange={(event) =>
              commit(row, event.currentTarget.value, event.currentTarget)}
          />
        {/if}
        <IconButton
          icon={X}
          label="Clear {row.label}"
          title="Use inherited value"
          onclick={() => useInherited(row)}
        />
      </span>
    </div>
    {#if error}
      <p
        class="error"
        id={errorId(row)}
        role="alert"
        data-setting-error={row.key}
      >
        {error}
      </p>
    {/if}
    <span class="meta">
      <!-- Said by the chip and the field; shown only for an unsaved edit. -->
      <span
        class="origin"
        class:visually-hidden={!edited}
        aria-hidden={edited ? undefined : "true"}
        data-origin={row.key}
        data-edited={edited || undefined}
      >
        {#if edited}
          {own === "" ? "Inherits" : show(own)} · not saved yet
        {:else if !effective}
          …
        {:else if applied}
          {show(applied.value)} · {describeOrigin(applied.origin, item)}
        {:else}
          {row.unset ?? "not set"}
        {/if}
      </span>
    </span>
    {#if row.note}
      <!-- Kept in the page when closed (hidden), so it can be found. -->
      <p
        id="{uid}-note-{row.key}"
        class="note"
        class:warn={row.key === "logging"}
        hidden={!warning && noteShown[row.key] !== true}
        data-setting-note={row.key}
      >
        {row.note}
      </p>
    {/if}
  </div>
{/snippet}

<!--
  The settings, the ones this item sets first; with `fold`, what it only
  inherits is behind a button.
-->
{#snippet rowsOf(rows: Row[], group: string, fold: boolean)}
  {@const isSet = (row: Row) =>
    row.own(settings) !== "" ||
    (saved !== undefined && row.own(saved) !== row.own(settings))}
  {@const mine = fold ? rows.filter(isSet) : rows}
  {@const inherited = fold ? rows.filter((row) => !isSet(row)) : []}
  {#each mine as row (row.key)}
    {@render settingRow(row)}
  {/each}
  {#if inherited.length > 0}
    <Button
      size="compact"
      variant="ghost"
      class="fold"
      data-inherited-toggle={group}
      aria-expanded={inheritedShown[group] === true}
      aria-controls="{uid}-{group}"
      onclick={() => (inheritedShown[group] = !inheritedShown[group])}
    >
      {#if inheritedShown[group]}
        <ChevronDown size={14} aria-hidden="true" />
      {:else}
        <ChevronRight size={14} aria-hidden="true" />
      {/if}
      {inheritedShown[group]
        ? "Hide inherited settings"
        : `Show ${inherited.length} inherited ${inherited.length === 1 ? "setting" : "settings"}`}
    </Button>
    <!-- Kept in the page when closed (hidden): each still says what applies. -->
    <div id="{uid}-{group}" class="inherited" hidden={!inheritedShown[group]}>
      {#each inherited as row (row.key)}
        {@render settingRow(row)}
      {/each}
    </div>
  {/if}
{/snippet}

{#if part === "device"}
  {@render rowsOf(device, "device", false)}
  {#if platformMatch}
    <p
      class="platform-match"
      data-platform-match={platformMatch.definition ?? ""}
      class:missing={platformMatch.definition === null}
    >
      {#if platformMatch.definition}
        Platform definition: {platformMatch.definition}
      {:else if platformMatch.broken}
        The platform definition {platformMatch.broken} has a pattern that cannot be
        used, so it is not used.
      {:else}
        No platform definition answers to “{platformMatch.platform}”.
      {/if}
      <button
        class="definitions"
        data-open-platforms
        onclick={() => openView("platforms")}>Definitions…</button
      >
    </p>
  {/if}
  <datalist id="platforms">
    {#each platformNames as name (name)}
      <option value={name}></option>
    {/each}
  </datalist>
{:else}
  {@render rowsOf(connection, "connection", true)}
  <datalist id="jump-hosts">
    {#each hostIds as id (id)}
      <option value={id}></option>
    {/each}
  </datalist>
  <datalist id="credential-profiles">
    {#each profileNames() as name (name)}
      <option value={name}></option>
    {/each}
  </datalist>

  {#if !serialUsed && !serialAsked}
    <Button
      size="compact"
      variant="ghost"
      class="fold"
      data-serial-add
      onclick={() => (serialAsked = true)}>Set up a serial line</Button
    >
  {/if}
  <!-- Kept in the page when unused (hidden): its settings still say what applies. -->
  <section
    aria-label="Serial line"
    data-serial-line
    hidden={!serialUsed && !serialAsked}
  >
    <h4>
      Serial line
      <Button
        size="compact"
        data-serial-refresh
        title="Look for serial ports again"
        onclick={refreshPorts}>Refresh ports</Button
      >
    </h4>
    {@render rowsOf(serial, "serial", true)}
    {#if portsError}
      <span class="origin" role="alert">{portsError}</span>
    {/if}
    <datalist id="serial-ports">
      {#each ports as port (port.name)}
        <option value={port.name}>{port.description || port.name}</option>
      {/each}
    </datalist>
    <datalist id="baud-rates">
      {#each BAUD_RATES as rate (rate)}
        <option value={rate}></option>
      {/each}
    </datalist>
  </section>

  <section aria-label="Web links">
    <h4>Web links</h4>
    {#each effective?.web_links ?? [] as link (link.value.name)}
      {@const own = isOwn(link.origin, item)}
      <div class="link" data-link={link.value.name}>
        <span class="name">{link.value.name}</span>
        {#if own}
          <span class="row">
            <input
              aria-label="{link.value.name} URL"
              value={link.value.url}
              spellcheck="false"
              onchange={async (event) => {
                const input = event.currentTarget;
                await changeLink(link.value.name, input.value);
                input.value =
                  settings.web_links.find((l) => l.name === link.value.name)
                    ?.url ?? "";
              }}
            />
            <IconButton
              icon={X}
              size="compact"
              label="Remove link {link.value.name}"
              onclick={() => removeLink(link.value.name)}
            />
          </span>
        {:else}
          <span class="url">{link.value.url}</span>
        {/if}
        <span class="origin">{describeOrigin(link.origin, item)}</span>
      </div>
    {/each}
    <form class="row" onsubmit={addLink}>
      <input
        class="link-name"
        name="new-link-name"
        placeholder="Name"
        spellcheck="false"
        bind:value={newLinkName}
      />
      <input
        name="new-link-url"
        placeholder={"https://{{address}}/"}
        title={"http or https; {{address}}, {{name}} and other fields are filled in"}
        spellcheck="false"
        bind:value={newLinkUrl}
      />
      <button
        type="submit"
        disabled={newLinkName.trim() === "" || newLinkUrl.trim() === ""}
        >Add</button
      >
    </form>
  </section>

  {#if (effective?.forwards ?? []).length > 0}
    <section aria-label="Port forwards" data-forwards-setting>
      <h4>Port forwards</h4>
      {#each effective?.forwards ?? [] as forward (forward.value.name)}
        <div class="link" data-forward={forward.value.name}>
          <span class="name">{forward.value.name}</span>
          <span class="url">{forward.value.forward}</span>
          <span class="origin">{describeOrigin(forward.origin, item)}</span>
        </div>
      {/each}
      <p class="note">
        Opened with each SSH session; set in the workspace files under
        <code>forwards</code>.
      </p>
    </section>
  {/if}
{/if}

<style>
  section {
    display: grid;
    gap: var(--space-1-5);
  }

  section[hidden],
  .inherited[hidden],
  .row[hidden],
  .note[hidden] {
    display: none;
  }

  .inherited {
    display: grid;
    gap: var(--space-1-5);
  }

  /* A disclosure, so it reads as something to press: a chevron, in the
     accent colour, lined up with the labels. */
  :global(.btn.ghost.compact.fold) {
    justify-self: start;
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    margin-left: calc(-1 * var(--space-1));
    padding: 0 var(--space-1);
    color: var(--accent);
    font-weight: var(--weight-medium);
  }

  .label {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    min-width: 0;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  /* Beside its label, the origin and the note sit under the field. */
  @container host-details (min-width: 560px) {
    .field {
      grid-template-columns: var(--label-col, 180px) minmax(0, 1fr);
      align-items: center;
      column-gap: var(--space-3);
    }

    .meta,
    .note,
    .error {
      margin-left: calc(var(--label-col, 180px) + var(--space-3));
    }

    .link {
      grid-template-columns: var(--label-col, 180px) minmax(0, 1fr);
      column-gap: var(--space-3);
      align-items: center;
    }

    .link .origin {
      grid-column: 2;
    }
  }

  h4 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 var(--space-1-5);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    color: var(--text);
  }

  .setting,
  .link {
    display: grid;
    row-gap: var(--space-0-5);
  }

  .field {
    display: grid;
    /* Rows only: the column gap beside the label is set above. */
    row-gap: var(--space-0-5);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .field > .label > label {
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--muted);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .row input,
  .row select {
    flex: 1;
    width: 100%;
  }

  /* What an inherited setting is, in place of its input: the value, where
     it comes from, and the way to give it a value of its own. */
  .chip-row {
    min-height: var(--control-height);
  }

  .chip {
    display: inline-flex;
    flex: 1;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 var(--space-2);
    min-width: 0;
    padding: var(--space-1) var(--space-2);
    background: var(--surface-raised);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }

  .chip-value {
    color: var(--text);
  }

  .chip-origin {
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  input[aria-invalid="true"],
  select[aria-invalid="true"] {
    border-color: var(--danger);
  }

  .origin {
    font-size: var(--text-2xs);
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  /* Help under a setting; the warning colour is kept for the one note
     that warns (logs can hold secrets). */
  .note {
    margin: var(--space-0-5) 0 0;
    padding: var(--space-1) var(--space-1-5);
    border-left: 2px solid var(--border-strong);
    font-size: var(--text-2xs);
    line-height: 1.5;
    color: var(--muted);
  }

  .note.warn {
    border-left-color: var(--warning);
    color: var(--text);
  }

  .platform-match {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-0-5) var(--space-2);
    margin: 0;
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .platform-match.missing {
    color: var(--warning);
  }

  .definitions {
    min-height: 0;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
  }

  .link .name {
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .url {
    overflow-wrap: anywhere;
  }

  .row .link-name {
    flex: 0 1 35%;
  }
</style>
