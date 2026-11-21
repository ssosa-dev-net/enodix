<script lang="ts">
  import X from "@lucide/svelte/icons/x";
  import Archive from "@lucide/svelte/icons/archive";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import FileCode from "@lucide/svelte/icons/file-code";
  import FileUp from "@lucide/svelte/icons/file-up";
  import FolderInput from "@lucide/svelte/icons/folder-input";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Logs from "@lucide/svelte/icons/logs";
  import Play from "@lucide/svelte/icons/play";
  import RotateCcwClock from "@lucide/svelte/icons/rotate-ccw-clock";
  import ScanSearch from "@lucide/svelte/icons/scan-search";
  import Trash from "@lucide/svelte/icons/trash";
  import CopyButton from "./CopyButton.svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import TagEditor from "./TagEditor.svelte";
  import { ENVIRONMENT_NAMES } from "./paste";
  import {
    jumpHostOf,
    otherWays as waysBesides,
    ownWayBlocked,
    PROTOCOL_NAMES,
  } from "./ways";
  import {
    effectiveSettings,
    hostRole,
    inventory,
    move,
    openBackup,
    openEnrich,
    openConfigHistory,
    openFiles,
    openJob,
    openScript,
    openSendFile,
    remove,
    renameTo,
    saveHost,
  } from "./inventory.svelte";
  import {
    allHosts,
    HOST_TEXT_FIELDS,
    hostWebLinks,
    openLogFolder,
    openWebLink,
    type CustomField,
    type Host,
    type HostWebLink,
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

  /** The ways in besides the host's own protocol. */
  const otherWays = $derived(
    effective.current ? waysBesides(host.id, effective.current) : [],
  );
  const ownWay = $derived(
    effective.current ? PROTOCOL_NAMES[effective.current.protocol.value] : null,
  );
  /** Why the host's own way in cannot be used (serial without a port). */
  const ownBlocked = $derived(ownWayBlocked(effective.current));
  const jumpHost = $derived(jumpHostOf(host.id, effective.current));
  const environment = $derived(effective.current?.environment?.value ?? null);

  /** The host's web links, filled from its fields by the core. */
  let webLinks = $state<HostWebLink[]>([]);
  /** Why the last web link clicked was not opened. */
  let webLinkError = $state<string | null>(null);
  /** Why the host's links could not be read, if they could not. */
  let webLinksUnavailable = $state<string | null>(null);

  $effect(() => {
    const id = host.id;
    // Any edit can change the links or the fields they are filled from.
    void inventory.workspace;
    let stale = false;
    hostWebLinks(id)
      .then((links) => {
        if (!stale) {
          webLinks = links;
          webLinksUnavailable = null;
        }
      })
      .catch((error) => {
        if (!stale) {
          // Never leave another host's links on screen.
          webLinks = [];
          webLinksUnavailable = `Cannot read the web links: ${error}`;
        }
      });
    return () => {
      stale = true;
    };
  });

  $effect(() => {
    void host.id;
    webLinkError = null;
  });

  /** Opens `link` in the browser; the core refuses what is not http(s). */
  function open(link: HostWebLink) {
    webLinkError = null;
    openWebLink(host.id, link.name).catch(
      (error) => (webLinkError = String(error)),
    );
  }

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
    <span class="facts">
      {#if environment}
        <span
          class="environment {environment}"
          data-details-environment={environment}
          >{ENVIRONMENT_NAMES[environment]}</span
        >
      {/if}
      <span class="address">{host.address}</span>
      {#if jumpHost}
        <span class="via" title="Reached through this jump host"
          >via {jumpHost}</span
        >
      {/if}
    </span>
    <div class="actions" data-ways-in aria-label="Ways in">
      <button
        class="primary"
        data-connect="default"
        disabled={ownBlocked !== null}
        title={ownBlocked ??
          (ownWay ? `Connect by ${ownWay}, the host's protocol` : "Connect")}
        onclick={() => onconnect(host)}>Connect</button
      >
      {#each otherWays as way (way.protocol)}
        <button
          data-connect={way.protocol}
          disabled={way.blocked !== null}
          title={way.blocked ??
            `Connect by ${PROTOCOL_NAMES[way.protocol]} instead`}
          onclick={() => onconnect(host, way.protocol)}
          >{PROTOCOL_NAMES[way.protocol]}</button
        >
      {/each}
      {#each webLinks as link (link.name)}
        <button
          class="web-link"
          class:refused={link.url === null}
          data-web-link={link.name}
          data-refused={link.url === null ? "" : undefined}
          title={link.url
            ? `Open ${link.url} in the browser`
            : `Not opened: ${link.refused}`}
          onclick={() => open(link)}
          >{link.name} <ExternalLink size={12} aria-hidden="true" /></button
        >
      {/each}
    </div>
    {#if webLinkError ?? webLinksUnavailable}
      <p class="error" role="alert" data-web-link-error>
        {webLinkError ?? webLinksUnavailable}
      </p>
    {/if}
    <div class="actions more" aria-label="Actions">
      <button
        data-open-logs
        title="Show this host's session logs in the file manager"
        onclick={() =>
          openLogFolder(host.id).catch(
            (error) => (inventory.editError = String(error)),
          )}
        ><Logs size={15} aria-hidden="true" /><span class="visually-hidden"
          >Open log folder</span
        ></button
      >
      {#if effective.current?.protocol.value === "ssh"}
        <button
          data-open-files
          title="Browse this host's files over SFTP, in a tab"
          onclick={() => openFiles(host.id)}
          ><FolderOpen size={15} aria-hidden="true" /><span
            class="visually-hidden">Files</span
          ></button
        >
      {/if}
      <button
        data-open-config-history
        title="Earlier versions of this host's configuration, kept by config backups"
        onclick={() => openConfigHistory(host.id)}
        ><RotateCcwClock size={15} aria-hidden="true" /><span
          class="visually-hidden">Config history</span
        ></button
      >
      <button
        data-host-back-up
        title="Fetch this host's configuration and keep it in its config history if it changed"
        onclick={() => openBackup({ kind: "hosts", ids: [host.id] }, host.name)}
        ><Archive size={15} aria-hidden="true" /><span class="visually-hidden"
          >Back up config</span
        ></button
      >
      <button
        data-host-enrich
        title="Ask the device for its vendor, model, serial number and OS version (NAPALM), then review what to fill in"
        onclick={() => openEnrich({ kind: "hosts", ids: [host.id] }, host.name)}
        ><ScanSearch size={15} aria-hidden="true" /><span
          class="visually-hidden">Fill in from device…</span
        ></button
      >
      <button
        data-host-run-snippet
        title="Run a saved snippet on this host as a job, without a terminal"
        onclick={() => openJob({ kind: "hosts", ids: [host.id] }, host.name)}
        ><Play size={15} aria-hidden="true" /><span class="visually-hidden"
          >Run snippet…</span
        ></button
      >
      <button
        data-host-run-script
        title="Run a Python script of the workspace for this host, as a job"
        onclick={() => openScript({ kind: "hosts", ids: [host.id] }, host.name)}
        ><FileCode size={15} aria-hidden="true" /><span class="visually-hidden"
          >Run script…</span
        ></button
      >
      <button
        data-host-send-file
        title="Send a file from this computer to this host, as a job"
        onclick={() =>
          openSendFile({ kind: "hosts", ids: [host.id] }, host.name)}
        ><FileUp size={15} aria-hidden="true" /><span class="visually-hidden"
          >Send file…</span
        ></button
      >
      <button
        title="Move this host to another folder"
        onclick={() => move(item)}
        ><FolderInput size={15} aria-hidden="true" /><span
          class="visually-hidden">Move…</span
        ></button
      >
      <button
        class="danger"
        title="Delete this host"
        onclick={() => remove(item)}
        ><Trash size={15} aria-hidden="true" /><span class="visually-hidden"
          >Delete…</span
        ></button
      >
    </div>
  </header>

  <section aria-label="Device" data-details-section="device">
    <h3>Device</h3>
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
    <SettingsPanel
      part="device"
      {item}
      settings={host.settings}
      effective={effective.current}
      hostIds={otherHosts}
      onchange={(change) =>
        save((current) => ({ settings: change(current.settings) }))}
    />
  </section>

  <section aria-label="Connection" data-details-section="connection">
    <h3>Connection</h3>
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
    <SettingsPanel
      part="connection"
      {item}
      settings={host.settings}
      effective={effective.current}
      hostIds={otherHosts}
      onchange={(change) =>
        save((current) => ({ settings: change(current.settings) }))}
    />
  </section>

  <div data-details-section="tags">
    <TagEditor
      tags={host.tags}
      effective={effective.current?.tags ?? null}
      {item}
      onchange={(change) => save((current) => ({ tags: change(current.tags) }))}
    />
  </div>

  <section aria-label="Custom fields" data-details-section="custom">
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
            }))}><X size={14} aria-hidden="true" /></button
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

  <section aria-label="Description" data-details-section="description">
    <h3>Description</h3>
    <span class="row">
      <textarea
        name="description"
        aria-label="Description"
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
  </section>
</div>

<style>
  .details {
    display: grid;
    gap: var(--space-4);
    padding: var(--space-3);
    font-size: var(--text-sm);
  }

  header {
    display: grid;
    gap: var(--space-1);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  h3 {
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

  .file {
    color: var(--muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  .facts {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1-5);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .facts .address {
    font-family: var(--font-mono);
  }

  .environment {
    padding: 0 var(--space-1-5);
    border-radius: var(--radius-sm);
    font-size: var(--text-2xs);
    font-weight: var(--weight-semibold);
    line-height: 1.6;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .environment.production {
    background: var(--env-production);
    color: var(--on-env-production);
  }

  .environment.staging {
    border: 1px solid var(--env-staging);
    color: var(--text);
  }

  .environment.lab {
    border: 1px dashed var(--env-lab);
    color: var(--text);
  }

  .actions.more {
    margin-top: var(--space-2);
    gap: var(--space-0-5);
  }

  /* One quiet row of icons: each button keeps its name for screen readers
     and shows it as its tooltip. */
  .actions.more button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .actions.more button:hover,
  .actions.more button:focus-visible {
    background: var(--surface-raised);
    color: var(--text);
  }

  .actions.more button.danger:hover,
  .actions.more button.danger:focus-visible {
    color: var(--danger);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    margin-top: var(--space-1);
  }

  .actions button,
  .small {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  .web-link.refused {
    color: var(--muted);
    text-decoration: line-through;
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  section {
    display: grid;
    gap: var(--space-1-5);
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
    padding: 0 var(--space-1-5);
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
