<script lang="ts">
  import X from "@lucide/svelte/icons/x";
  import {
    describeOrigin,
    inventory,
    isOwn,
    platformDefinition,
    type Item,
  } from "./inventory.svelte";
  import { profileNames } from "./profiles.svelte";
  import { openView } from "./views";
  import { serialPorts, type SerialPortInfo } from "./session";
  import {
    MAX_PASTE_LINE_DELAY_MS,
    type Effective,
    type EffectiveSettings,
    type Folder,
    type Settings,
  } from "./workspace";

  interface Props {
    item: Item;
    /** What the host or folder sets itself. */
    settings: Settings;
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

  let { item, settings, effective, hostIds, onchange, part }: Props = $props();

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

  const BAUD_RATES = [
    1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400,
  ];

  /**
   * Saves a setting edited in `control`; afterwards the control shows what
   * the file holds, which is the old value if the edit was refused.
   */
  async function commit(
    row: Row,
    value: string,
    control?: HTMLInputElement | HTMLSelectElement,
  ) {
    await onchange((current) => row.set(current, value));
    if (control) control.value = row.own(settings);
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

{#snippet rowsOf(rows: Row[])}
  {#each rows as row (row.key)}
    {@const own = row.own(settings)}
    {@const applied = effective ? row.effective(effective) : null}
    <div class="setting" data-setting={row.key}>
      <label class="field">
        <span>{row.label}</span>
        <span class="row">
          {#if row.input.kind === "choice"}
            <select
              name="setting-{row.key}"
              value={own}
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
              name="setting-{row.key}"
              type={row.input.kind}
              min={row.input.kind === "number" ? row.input.min : undefined}
              max={row.input.kind === "number" ? row.input.max : undefined}
              list={row.input.list}
              spellcheck="false"
              value={own}
              placeholder={own === "" && applied ? show(applied.value) : ""}
              onchange={(event) =>
                commit(row, event.currentTarget.value, event.currentTarget)}
            />
          {/if}
          {#if own !== ""}
            <button
              class="remove"
              aria-label="Clear {row.label}"
              title="Clear: inherit from above"
              onclick={() => commit(row, "")}
              ><X size={14} aria-hidden="true" /></button
            >
          {/if}
        </span>
      </label>
      <span class="origin" data-origin={row.key}>
        {#if !effective}
          …
        {:else if applied}
          {show(applied.value)} · {describeOrigin(applied.origin, item)}
        {:else}
          {row.unset ?? "not set"}
        {/if}
      </span>
      {#if row.note}
        <p class="note" data-setting-note={row.key}>{row.note}</p>
      {/if}
    </div>
  {/each}
{/snippet}

{#if part === "device"}
  {@render rowsOf(device)}
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
  {@render rowsOf(connection)}
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

  <section aria-label="Serial line">
    <h4>
      Serial line
      <button
        class="small"
        data-serial-refresh
        title="Look for serial ports again"
        onclick={refreshPorts}>Refresh ports</button
      >
    </h4>
    {@render rowsOf(serial)}
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
            <button
              class="remove"
              aria-label="Remove link {link.value.name}"
              onclick={() => removeLink(link.value.name)}
              ><X size={14} aria-hidden="true" /></button
            >
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
        class="small"
        disabled={newLinkName.trim() === "" || newLinkUrl.trim() === ""}
        >Add</button
      >
    </form>
  </section>
{/if}

<style>
  section {
    display: grid;
    gap: var(--space-1-5);
  }

  h4 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 var(--space-1-5);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .setting,
  .link {
    display: grid;
    gap: var(--space-0-5);
  }

  .field {
    display: grid;
    gap: var(--space-0-5);
    font-size: var(--text-xs);
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

  .origin {
    font-size: var(--text-2xs);
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  .note {
    margin: var(--space-0-5) 0 0;
    padding: var(--space-1) var(--space-1-5);
    border-left: 2px solid var(--warning);
    font-size: var(--text-2xs);
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

  .small {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  .remove {
    padding: 0 var(--space-1-5);
    border: 0;
    background: none;
    color: var(--muted);
  }

  .remove:hover:not(:disabled) {
    color: var(--danger);
  }
</style>
