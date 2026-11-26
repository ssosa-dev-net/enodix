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
  import SquarePen from "@lucide/svelte/icons/square-pen";
  import Trash from "@lucide/svelte/icons/trash";
  import IconButton from "./IconButton.svelte";
  import MenuButton, { type MenuEntry } from "./MenuButton.svelte";
  import CopyButton from "./CopyButton.svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import TagEditor from "./TagEditor.svelte";
  import { ENVIRONMENT_NAMES } from "./paste";
  import { EDIT_HOST_SHORTCUT, openHostEditor } from "./views";
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
    /** The host as shown: as saved, or with the editor's unsaved edits. */
    host: Host;
    /** Connects over `protocol`, or over the host's own if none is given. */
    onconnect: (host: Host, protocol?: Protocol) => void;
    /**
     * Where edits go (the host editor's draft, #10). Without it, each edit
     * is saved at once (the details sheet), the name by renaming.
     */
    onedit?: (
      changes: Partial<Host> | ((current: Host) => Partial<Host>),
    ) => void;
    /** The host as saved, when `host` shows unsaved edits: the settings
     * tell an edited setting from a saved one by it. */
    saved?: Host;
    /**
     * The short read-only form (the details sheet, #10): the header with
     * its actions, a few facts and an Edit host button, in place of the
     * fields. Edits are made in the host editor tab.
     */
    summary?: boolean;
  }

  let { host, onconnect, onedit, saved, summary = false }: Props = $props();

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
    effective.current ? waysBesides(effective.current) : [],
  );
  const ownWay = $derived(
    effective.current ? PROTOCOL_NAMES[effective.current.protocol.value] : null,
  );
  /** Why the host's own way in cannot be used (serial without a port). */
  const ownBlocked = $derived(ownWayBlocked(effective.current));
  const jumpHost = $derived(jumpHostOf(host.id, effective.current));
  const environment = $derived(effective.current?.environment?.value ?? null);

  const target = $derived({ kind: "hosts" as const, ids: [host.id] });

  /** Where the host's own things open: tabs and the file manager. */
  const openEntries = $derived<MenuEntry[]>([
    ...(effective.current?.protocol.value === "ssh"
      ? [
          {
            label: "Files",
            icon: FolderOpen,
            title: "Browse this host's files over SFTP, in a tab",
            attributes: { "data-open-files": "" },
            run: () => openFiles(host.id),
          },
        ]
      : []),
    {
      label: "Session logs",
      icon: Logs,
      title: "Show this host's session logs in the file manager",
      attributes: { "data-open-logs": "" },
      run: () =>
        openLogFolder(host.id).catch(
          (error) => (inventory.editError = String(error)),
        ),
    },
    {
      label: "Config history",
      icon: RotateCcwClock,
      title:
        "Earlier versions of this host's configuration, kept by config backups",
      attributes: { "data-open-config-history": "" },
      run: () => openConfigHistory(host.id),
    },
  ]);

  /** What runs on the host as a job, without a terminal. */
  const runEntries = $derived<MenuEntry[]>([
    {
      label: "Snippet…",
      icon: Play,
      title: "Run a saved snippet on this host as a job, without a terminal",
      attributes: { "data-host-run-snippet": "" },
      run: () => openJob(target, host.name),
    },
    {
      label: "Script…",
      icon: FileCode,
      title: "Run a Python script of the workspace for this host, as a job",
      attributes: { "data-host-run-script": "" },
      run: () => openScript(target, host.name),
    },
    {
      label: "Send file…",
      icon: FileUp,
      title: "Send a file from this computer to this host, as a job",
      attributes: { "data-host-send-file": "" },
      run: () => openSendFile(target, host.name),
    },
    {
      label: "Back up config",
      icon: Archive,
      title:
        "Fetch this host's configuration and keep it in its config history if it changed",
      attributes: { "data-host-back-up": "" },
      run: () => openBackup(target, host.name),
    },
    {
      label: "Fill in from device…",
      icon: ScanSearch,
      title:
        "Ask the device for its vendor, model, serial number and OS version (NAPALM), then review what to fill in",
      attributes: { "data-host-enrich": "" },
      run: () => openEnrich(target, host.name),
    },
  ]);

  const moreEntries = $derived<MenuEntry[]>([
    {
      label: "Move…",
      icon: FolderInput,
      title: "Move this host to another folder",
      attributes: { "data-host-move": "" },
      run: () => move(item),
    },
    { separator: true },
    {
      label: "Delete…",
      icon: Trash,
      danger: true,
      title: "Delete this host",
      attributes: { "data-host-delete": "" },
      run: () => remove(item),
    },
  ]);

  /** What the short form lists: the fields that say what the device is,
   * and the settings that are inherited (the values that apply). */
  const summaryRows = $derived<[string, string, string | null][]>([
    ["vendor", "Vendor", host.vendor || null],
    ["model", "Model", host.model || null],
    ["site", "Site", host.site || null],
    ["platform", "Platform", effective.current?.platform?.value ?? null],
    [
      "credential_profile",
      "Credential profile",
      effective.current?.credential_profile?.value ?? null,
    ],
  ]);

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

  /** The sections the list on the left jumps to, in page order. */
  const SECTIONS = [
    ["device", "Device"],
    ["location", "Location"],
    ["connection", "Connection"],
    ["tags", "Tags"],
    ["custom", "Custom fields"],
    ["description", "Description"],
  ] as const;
  /** The fields that say where the device is; the rest describe it. */
  const LOCATION_KEYS: readonly string[] = [
    "site",
    "building",
    "room",
    "rack",
    "rack_unit",
  ];
  const deviceFields = HOST_TEXT_FIELDS.filter(
    ([key]) => !LOCATION_KEYS.includes(key),
  );
  const locationFields = HOST_TEXT_FIELDS.filter(([key]) =>
    LOCATION_KEYS.includes(key),
  );

  let root = $state<HTMLElement | null>(null);
  let current = $state<string>("device");

  function jumpTo(id: string) {
    current = id;
    root
      ?.querySelector(`[data-details-section="${id}"]`)
      ?.scrollIntoView({ block: "start", behavior: "smooth" });
  }

  // The section nearest the top of the scrolling area is the current one.
  $effect(() => {
    if (!root || typeof IntersectionObserver === "undefined") return;
    const sections = [...root.querySelectorAll("[data-details-section]")];
    const observer = new IntersectionObserver(
      (entries) => {
        const top = entries
          .filter((entry) => entry.isIntersecting)
          .sort(
            (a, b) => a.boundingClientRect.top - b.boundingClientRect.top,
          )[0];
        const id = (top?.target as HTMLElement | undefined)?.dataset
          .detailsSection;
        if (id) current = id;
      },
      { rootMargin: "0px 0px -70% 0px" },
    );
    sections.forEach((section) => observer.observe(section));
    return () => observer.disconnect();
  });

  let newKey = $state("");
  let newValue = $state("");

  /** Saves the host with some fields changed. */
  function save(changes: Partial<Host> | ((current: Host) => Partial<Host>)) {
    if (onedit) {
      onedit(changes);
      return Promise.resolve();
    }
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

<div class="details" class:summary aria-label="Host details" bind:this={root}>
  <header>
    <h2>{host.name}</h2>
    {#if !summary}
      <span class="file" title="The host's file in the workspace"
        >{host.id}</span
      >
    {/if}
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
    <div class="actions menus" aria-label="Actions">
      <MenuButton label="Open" entries={openEntries} data-menu-open />
      <MenuButton label="Run" entries={runEntries} data-menu-run />
      <MenuButton label="More" entries={moreEntries} data-menu-more />
    </div>
  </header>

  {#if summary}
    <dl class="facts-list" data-details-summary>
      {#each summaryRows as [key, label, value] (key)}
        <dt>{label}</dt>
        <dd data-summary={key} class:unset={value === null}>
          {value ?? "Not set"}
        </dd>
      {/each}
    </dl>
    <button
      class="primary edit-host"
      data-details-edit
      aria-keyshortcuts="Control+Shift+E"
      title="Edit this host in a tab beside the terminals, with room for every field ({EDIT_HOST_SHORTCUT})"
      onclick={openHostEditor}
    >
      <span class="edit-label"
        ><SquarePen size={14} aria-hidden="true" /> Edit host</span
      >
      <kbd>{EDIT_HOST_SHORTCUT}</kbd>
    </button>
  {:else}
    <div class="layout">
      <nav class="sections-nav" aria-label="Host sections">
        {#each SECTIONS as [id, label] (id)}
          <button
            type="button"
            data-section-link={id}
            aria-current={current === id ? "true" : undefined}
            onclick={() => jumpTo(id)}>{label}</button
          >
        {/each}
      </nav>
      <div class="form">
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
                  if (onedit) onedit({ name: input.value });
                  else await renameTo(item, input.value);
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
                  await save({
                    role: select.value === "" ? null : select.value,
                  });
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
          {#each deviceFields as [key, label] (key)}
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
                <CopyButton
                  value={host[key] ?? ""}
                  what={label.toLowerCase()}
                />
              </span>
            </label>
          {/each}
          <SettingsPanel
            part="device"
            {item}
            settings={host.settings}
            saved={saved?.settings}
            effective={effective.current}
            hostIds={otherHosts}
            onchange={(change) =>
              save((current) => ({ settings: change(current.settings) }))}
          />
        </section>

        <section aria-label="Location" data-details-section="location">
          <h3>Location</h3>
          {#each locationFields as [key, label] (key)}
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
                <CopyButton
                  value={host[key] ?? ""}
                  what={label.toLowerCase()}
                />
              </span>
            </label>
          {/each}
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
            saved={saved?.settings}
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
            onchange={(change) =>
              save((current) => ({ tags: change(current.tags) }))}
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
              <IconButton
                icon={X}
                size="compact"
                label="Remove field {field.key}"
                onclick={() =>
                  save((current) => ({
                    custom: current.custom.filter((f) => f.key !== field.key),
                  }))}
              />
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
            <button type="submit" disabled={newKey.trim() === ""}>Add</button>
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
    </div>
  {/if}
</div>

<style>
  .details {
    /* The label column and the widest the form grows, when there is room. */
    --label-col: 180px;
    --form-max: 720px;
    --nav-col: 10rem;
    container: host-details / inline-size;
    display: grid;
    gap: var(--space-4);
    padding: var(--space-3);
    font-size: var(--text-sm);
  }

  /* The host editor caps its children at 48rem; this sizes itself (the
     header and form below), so the list of sections fits beside the form. */
  .details.details {
    max-width: none;
  }

  header {
    display: grid;
    gap: var(--space-1);
  }

  h2 {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  h3 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 var(--space-1-5);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    color: var(--text);
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

  /* The grouped actions (Open, Run, More) in a row of their own. */
  .actions.menus {
    margin-top: var(--space-2);
    gap: var(--space-1);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    margin-top: var(--space-1);
  }

  .actions button {
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
    font-size: var(--text-xs);
  }

  .details.summary {
    gap: var(--space-3);
  }

  /* The short form: what the device is, label beside value. */
  .facts-list {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--space-1) var(--space-3);
    margin: 0;
    font-size: var(--text-sm);
  }

  .facts-list dt {
    color: var(--muted);
  }

  .facts-list dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .facts-list dd.unset {
    color: var(--muted);
  }

  .edit-host {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    width: 100%;
  }

  .edit-label {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1-5);
  }

  /* On the primary button: the button's own text colour, not the
     default kbd style, which is dark on the accent. */
  .edit-host kbd {
    font-family: var(--font-ui);
    font-size: var(--text-xs);
    font-weight: var(--weight-regular, 400);
    color: inherit;
    background: none;
    border: 1px solid currentColor;
    border-radius: var(--radius-sm);
    padding: 0 var(--space-1);
    opacity: 0.9;
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

  .field > span:first-child {
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

  .layout {
    display: grid;
    gap: var(--space-4);
  }

  .form {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
  }

  .sections-nav {
    display: none;
  }

  /* Room for it (the host editor, not the narrow details sheet): the
     header and form stay as wide as they need, the label goes beside its
     field. Below this, one column, as in the sheet. */
  @container host-details (min-width: 560px) {
    header,
    .layout {
      width: 100%;
      max-width: var(--form-max);
      margin-inline: auto;
    }

    .field {
      grid-template-columns: var(--label-col) minmax(0, 1fr);
      align-items: center;
      column-gap: var(--space-3);
    }

    .custom .key {
      flex-basis: var(--label-col);
    }
  }

  /* Wide enough for the list of sections beside the form. */
  @container host-details (min-width: 760px) {
    header,
    .layout {
      max-width: calc(var(--nav-col) + var(--space-4) + var(--form-max));
    }

    .layout {
      grid-template-columns: var(--nav-col) minmax(0, 1fr);
      align-items: start;
    }

    .sections-nav {
      position: sticky;
      top: var(--space-3);
      display: grid;
      gap: var(--space-0-5);
    }

    .sections-nav button {
      justify-content: flex-start;
      min-height: var(--control-height-small);
      padding: 0 var(--space-2);
      border-color: transparent;
      background: none;
      color: var(--muted);
      font-size: var(--text-sm);
      text-align: left;
    }

    .sections-nav button:hover {
      color: var(--text);
    }

    .sections-nav button[aria-current="true"] {
      border-left: 2px solid var(--accent);
      color: var(--text);
      font-weight: var(--weight-semibold);
    }
  }
</style>
