<script lang="ts">
  import CopyButton from "./CopyButton.svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import TagEditor from "./TagEditor.svelte";
  import {
    effectiveSettings,
    hostRole,
    inventory,
    move,
    remove,
    renameTo,
    saveHost,
  } from "./inventory.svelte";
  import {
    allHosts,
    HOST_TEXT_FIELDS,
    openLogFolder,
    type CustomField,
    type Host,
    type Protocol,
  } from "./workspace";

  interface Props {
    host: Host;
    /** Connects over `protocol`, or over the host's own if none is given. */
    onconnect: (host: Host, protocol?: Protocol) => void;
  }

  let { host, onconnect }: Props = $props();

  const item = $derived({ kind: "host" as const, id: host.id });
  const effective = effectiveSettings(() => item);
  const roles = $derived(inventory.workspace?.roles ?? []);
  const role = $derived(hostRole(host.id));
  const otherHosts = $derived(
    inventory.workspace
      ? allHosts(inventory.workspace.tree)
          .map((h) => h.id)
          .filter((id) => id !== host.id)
      : [],
  );

  const protocolNames: Record<Protocol, string> = {
    ssh: "SSH",
    telnet: "Telnet",
    serial: "Serial",
  };
  /** The ways in besides the host's own protocol. */
  const otherWays = $derived(
    (["ssh", "telnet", "serial"] as const).filter(
      (protocol) => protocol !== effective.current?.protocol.value,
    ),
  );
  const ownWay = $derived(
    effective.current ? protocolNames[effective.current.protocol.value] : null,
  );
  /**
   * The jump host the host is reached through, which telnet cannot use
   * (the core refuses it). A host that inherits itself as its jump host
   * connects directly.
   */
  const jumpHost = $derived.by(() => {
    const jump = effective.current?.jump_host?.value;
    return jump && jump.toLowerCase() !== host.id.toLowerCase() ? jump : null;
  });

  /** Serial needs a port, which nothing sets by default. */
  const noSerialPort = $derived(
    effective.current !== null && effective.current.serial.port === null,
  );

  /** The host's own way in is serial, but no port is set. */
  const serialWithoutPort = $derived(
    noSerialPort && effective.current?.protocol.value === "serial",
  );

  let newKey = $state("");
  let newValue = $state("");

  /** Saves the host with some fields changed. */
  function save(changes: Partial<Host> | ((current: Host) => Partial<Host>)) {
    return saveHost(host.id, changes);
  }

  type Field = HTMLInputElement | HTMLTextAreaElement;

  /**
   * Saves a field edited in `input`; afterwards the input shows what the
   * file holds, which is the old value if the edit was refused.
   */
  async function commit(
    event: Event,
    changes: (
      value: string,
    ) => Partial<Host> | ((current: Host) => Partial<Host>),
    stored: () => string,
  ) {
    const input = event.currentTarget as Field;
    await save(changes(input.value));
    input.value = stored();
  }

  /** Changes the custom field named `key`, wherever it is by then. */
  function changeCustom(key: string, change: Partial<CustomField>) {
    return (current: Host) => ({
      custom: current.custom.map((field) =>
        field.key === key ? { ...field, ...change } : field,
      ),
    });
  }

  function addCustom(event: SubmitEvent) {
    event.preventDefault();
    const field = { key: newKey, value: newValue };
    save((current) => ({ custom: [...current.custom, field] })).then(() => {
      if (host.custom.some((saved) => saved.key === field.key.trim())) {
        newKey = "";
        newValue = "";
      }
    });
  }
</script>

<div class="details" aria-label="Host details">
  <header>
    <h2>{host.name}</h2>
    <span class="file" title="The host's file in the workspace">{host.id}</span>
    <div class="actions">
      <button
        data-connect="default"
        disabled={serialWithoutPort}
        title={serialWithoutPort
          ? "Choose the serial port in the serial line settings first"
          : ownWay
            ? `Connect by ${ownWay}, the host's protocol`
            : "Connect"}
        onclick={() => onconnect(host)}>Connect</button
      >
      {#each otherWays as protocol (protocol)}
        {@const blocked =
          (protocol === "telnet" && jumpHost !== null) ||
          (protocol === "serial" && noSerialPort)}
        <button
          data-connect={protocol}
          disabled={blocked}
          title={!blocked
            ? `Connect by ${protocolNames[protocol]} instead`
            : protocol === "serial"
              ? "Choose the serial port in the serial line settings first"
              : `Telnet cannot go through the jump host ${jumpHost}`}
          onclick={() => onconnect(host, protocol)}
          >{protocolNames[protocol]}</button
        >
      {/each}
      <button
        data-open-logs
        title="Show this host's session logs in the file manager"
        onclick={() =>
          openLogFolder(host.id).catch(
            (error) => (inventory.editError = String(error)),
          )}>Open log folder</button
      >
      <button onclick={() => move(item)}>Move…</button>
      <button class="danger" onclick={() => remove(item)}>Delete…</button>
    </div>
  </header>

  <section>
    <label class="field">
      <span>Name</span>
      <span class="row">
        <input
          name="name"
          value={host.name}
          spellcheck="false"
          onchange={async (event) => {
            const input = event.currentTarget;
            await renameTo(item, input.value);
            input.value = host.name;
          }}
        />
        <CopyButton value={host.name} what="name" />
      </span>
    </label>
    <label class="field">
      <span>Management address</span>
      <span class="row">
        <input
          name="address"
          value={host.address}
          spellcheck="false"
          onchange={(event) =>
            commit(
              event,
              (address) => ({ address }),
              () => host.address,
            )}
        />
        <CopyButton value={host.address} what="management address" />
      </span>
    </label>
    <label class="field">
      <span>Role</span>
      <span class="row">
        {#if role}
          <RoleIcon {role} />
        {/if}
        <select
          name="role"
          value={role?.id ?? ""}
          onchange={async (event) => {
            const select = event.currentTarget;
            await save({ role: select.value === "" ? null : select.value });
            select.value = role?.id ?? "";
          }}
        >
          <option value="">No role</option>
          {#each roles as option (option.id)}
            <option value={option.id}>{option.name}</option>
          {/each}
          {#if role?.source === "undefined"}
            <option value={role.id}>{role.name} (not defined)</option>
          {/if}
        </select>
      </span>
    </label>
    {#each HOST_TEXT_FIELDS as [key, label] (key)}
      <label class="field">
        <span>{label}</span>
        <span class="row">
          <input
            name={key}
            value={host[key] ?? ""}
            spellcheck="false"
            onchange={(event) =>
              commit(
                event,
                (value) => ({ [key]: value }),
                () => host[key] ?? "",
              )}
          />
          <CopyButton value={host[key] ?? ""} what={label.toLowerCase()} />
        </span>
      </label>
    {/each}
    <label class="field">
      <span>Description</span>
      <span class="row">
        <textarea
          name="description"
          rows="3"
          value={host.description ?? ""}
          onchange={(event) =>
            commit(
              event,
              (description) => ({ description }),
              () => host.description ?? "",
            )}></textarea>
        <CopyButton value={host.description ?? ""} what="description" />
      </span>
    </label>
  </section>

  <TagEditor
    tags={host.tags}
    effective={effective.current?.tags ?? null}
    {item}
    onchange={(change) => save((current) => ({ tags: change(current.tags) }))}
  />

  <SettingsPanel
    {item}
    settings={host.settings}
    effective={effective.current}
    hostIds={otherHosts}
    onchange={(change) =>
      save((current) => ({ settings: change(current.settings) }))}
  />

  <section>
    <h3>Custom fields</h3>
    {#each host.custom as field (field.key)}
      <div class="row custom">
        <input
          class="key"
          aria-label="Field name"
          value={field.key}
          spellcheck="false"
          onchange={(event) =>
            commit(
              event,
              (key) => changeCustom(field.key, { key }),
              () => field.key,
            )}
        />
        <input
          class="value"
          aria-label="{field.key} value"
          data-custom={field.key}
          value={field.value}
          spellcheck="false"
          onchange={(event) =>
            commit(
              event,
              (value) => changeCustom(field.key, { value }),
              () => field.value,
            )}
        />
        <CopyButton value={field.value} what={field.key} />
        <button
          class="remove"
          aria-label="Remove field {field.key}"
          onclick={() =>
            save((current) => ({
              custom: current.custom.filter((f) => f.key !== field.key),
            }))}>×</button
        >
      </div>
    {/each}
    <form class="row custom" onsubmit={addCustom}>
      <input
        class="key"
        name="new-key"
        placeholder="Name"
        spellcheck="false"
        bind:value={newKey}
      />
      <input
        class="value"
        name="new-value"
        placeholder="Value"
        spellcheck="false"
        bind:value={newValue}
      />
      <button type="submit" class="small" disabled={newKey.trim() === ""}
        >Add</button
      >
    </form>
  </section>
</div>

<style>
  .details {
    display: grid;
    gap: 1rem;
    padding: 0.8rem;
    font-size: 0.85rem;
  }

  header {
    display: grid;
    gap: 0.3rem;
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  h3 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 0.4rem;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .file {
    color: var(--muted);
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.3rem;
  }

  .actions button,
  .small {
    padding: 0.2rem 0.6rem;
    font-size: 0.8rem;
  }

  section {
    display: grid;
    gap: 0.45rem;
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
  .row select,
  .row textarea {
    flex: 1;
    width: 100%;
  }

  textarea {
    resize: vertical;
  }

  .custom .key {
    flex: 0 1 40%;
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

  .danger:hover:not(:disabled) {
    color: var(--danger);
  }
</style>
