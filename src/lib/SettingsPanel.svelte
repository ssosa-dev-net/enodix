<script lang="ts">
  import { describeOrigin, isOwn, type Item } from "./inventory.svelte";
  import { profileNames } from "./profiles.svelte";
  import type { Effective, EffectiveSettings, Settings } from "./workspace";

  interface Props {
    item: Item;
    /** What the host or folder sets itself. */
    settings: Settings;
    /** What applies to it (null while loading). */
    effective: EffectiveSettings | null;
    /** Host ids a jump host can be chosen from. */
    hostIds: string[];
    onchange: (change: (current: Settings) => Settings) => Promise<void>;
  }

  let { item, settings, effective, hostIds, onchange }: Props = $props();

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
    },
    {
      key: "jump_host",
      label: "Jump host",
      input: { kind: "text", list: "jump-hosts" },
      own: (s) => s.jump_host ?? "",
      set: (s, v) => ({ ...s, jump_host: toText(v) }),
      effective: (e) => e.jump_host,
      unset: "none (direct)",
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
      key: "logging",
      label: "Logging",
      input: { kind: "choice", options: ON_OFF },
      own: (s) => flag(s.logging),
      set: (s, v) => ({ ...s, logging: toFlag(v) }),
      effective: (e) => e.logging,
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
  });
  const choices = (...values: string[]): Options =>
    values.map((value) => [value, value]);

  const serial: Row[] = [
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
        options: choices("none", "odd", "even", "mark", "space"),
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
              onclick={() => commit(row, "")}>×</button
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
    </div>
  {/each}
{/snippet}

<section aria-label="Connection settings">
  <h3>Connection</h3>
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
</section>

<section aria-label="Serial line">
  <h3>Serial line</h3>
  {@render rowsOf(serial)}
  <datalist id="baud-rates">
    {#each BAUD_RATES as rate (rate)}
      <option value={rate}></option>
    {/each}
  </datalist>
</section>

<section aria-label="Web links">
  <h3>Web links</h3>
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
            onclick={() => removeLink(link.value.name)}>×</button
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
      placeholder="https://…"
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

<style>
  section {
    display: grid;
    gap: 0.45rem;
  }

  h3 {
    margin: 0 0 0.4rem;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .setting,
  .link {
    display: grid;
    gap: 0.1rem;
  }

  .field {
    display: grid;
    gap: 0.15rem;
    font-size: 0.75rem;
    color: var(--muted);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .row input,
  .row select {
    flex: 1;
    width: 100%;
  }

  .origin {
    font-size: 0.7rem;
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  .link .name {
    font-size: 0.75rem;
    color: var(--muted);
  }

  .url {
    overflow-wrap: anywhere;
  }

  .row .link-name {
    flex: 0 1 35%;
  }

  .small {
    padding: 0.2rem 0.6rem;
    font-size: 0.8rem;
  }

  .remove {
    padding: 0 0.45rem;
    border: 0;
    background: none;
    color: var(--muted);
  }

  .remove:hover:not(:disabled) {
    color: var(--danger);
  }
</style>
