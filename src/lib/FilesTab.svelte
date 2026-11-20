<script lang="ts">
  // A host's files over SFTP, in its tab (#103; one tab per host, main
  // window only). It connects when it opens, with the host's stored profile
  // or a login typed here; host-key and login questions are asked over the
  // tab with the terminal's components. It lists one folder at a time and
  // renames, deletes and makes folders on the device; on a production host
  // each of those asks first, Cancel focused. Its connection closes with
  // the tab.
  import { onDestroy, onMount, tick } from "svelte";
  import ArrowUp from "@lucide/svelte/icons/arrow-up";
  import FileIcon from "@lucide/svelte/icons/file";
  import Folder from "@lucide/svelte/icons/folder";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Link from "@lucide/svelte/icons/link";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import HostKeyCheck from "./HostKeyCheck.svelte";
  import LoginQuestions from "./LoginQuestions.svelte";
  import { hostLogin } from "./credentials";
  import {
    answerFiles,
    cancelFiles,
    closeFiles,
    decideFilesHostKey,
    deleteFile,
    listFiles,
    makeFolder,
    openFiles,
    renameFile,
    sizeText,
    timeText,
    type BrowserEvent,
    type Listing,
    type RemoteEntry,
  } from "./files";
  import { hostSafety, inventory } from "./inventory.svelte";
  import type {
    HostKeyDecision,
    HostKeyQuestion,
    LoginPrompt,
  } from "./session";
  import { findHost } from "./workspace";

  interface Props {
    /** The host whose files are shown. */
    host: string;
  }

  let { host }: Props = $props();

  const name = $derived(
    (inventory.workspace && findHost(inventory.workspace.tree, host)?.name) ??
      host,
  );
  const production = $derived(hostSafety(host)?.environment === "production");

  type Phase =
    | { kind: "starting" }
    /** The host has no stored profile: a login is typed here. */
    | { kind: "credentials"; why: string | null }
    | { kind: "connecting" }
    | { kind: "connected" }
    | { kind: "noSftp"; reason: string }
    | { kind: "failed"; reason: string }
    | { kind: "lost"; reason: string };

  let phase = $state<Phase>({ kind: "starting" });
  /** The open browser's id, once the shell has given it. */
  let browser = $state<number | null>(null);
  /** Counts browsers opened, so a closed one's late events are dropped. */
  let generation = 0;
  /** The login typed for this tab once it got in, kept for Reconnect. */
  let typed: { username: string; password: string } | null = null;
  /** The login being tried. */
  let trying: { username: string; password: string } | null = null;

  let hostKey = $state<{ question: HostKeyQuestion; key: number } | null>(null);
  let prompt = $state<{ prompt: LoginPrompt; key: number } | null>(null);
  let questions = 0;

  let listing = $state<Listing | null>(null);
  let path = $state("");
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  /** A name being typed: for a new folder, or a rename of `entry`. */
  let naming = $state<{ entry: RemoteEntry | null; value: string } | null>(
    null,
  );
  /** A change on a production host, waiting for its answer. */
  let confirming = $state<{
    text: string;
    label: string;
    run: () => void;
  } | null>(null);
  let nameInput = $state<HTMLInputElement>();
  let cancelButton = $state<HTMLButtonElement>();

  const LOGIN_PROMPT: LoginPrompt = {
    name: "",
    instructions: "",
    questions: [
      { text: "Username", echo: true },
      { text: "Password", echo: false },
    ],
  };

  onMount(() => void start(null));
  onDestroy(() => {
    generation += 1;
    if (browser !== null) void closeFiles(browser);
  });

  /** Connects as the host logs in, starting in `folder` (else where the
   * host's platform puts files). A typed login is kept for reconnects. */
  async function start(folder: string | null) {
    if (typed) return open(typed, folder);
    phase = { kind: "starting" };
    let login;
    try {
      login = await hostLogin(host, "ssh");
    } catch (reason) {
      phase = { kind: "failed", reason: String(reason) };
      return;
    }
    if (login.kind === "profile") void open(null, folder);
    else
      phase = {
        kind: "credentials",
        why:
          login.kind === "notSetUp"
            ? `The profile ${login.profile} is not set up on this computer.`
            : null,
      };
  }

  /** Opens a new browser (closing the last), starting in `folder`. */
  async function open(
    credentials: { username: string; password: string } | null,
    folder: string | null,
  ) {
    const old = browser;
    browser = null;
    if (old !== null) void closeFiles(old);
    generation += 1;
    const mine = generation;
    trying = credentials;
    hostKey = null;
    prompt = null;
    naming = null;
    confirming = null;
    error = null;
    phase = { kind: "connecting" };
    try {
      const id = await openFiles(host, credentials, folder, (event) => {
        if (mine === generation) onevent(event);
      });
      if (mine === generation) browser = id;
      else void closeFiles(id);
    } catch (reason) {
      if (mine === generation)
        phase = { kind: "failed", reason: String(reason) };
    }
  }

  function onevent(event: BrowserEvent) {
    switch (event.kind) {
      case "hostKey":
        hostKey = { question: event.question, key: ++questions };
        break;
      case "prompt":
        prompt = { prompt: event.prompt, key: ++questions };
        break;
      case "connected":
        typed = trying;
        show(event.listing);
        notice = event.notice;
        phase = { kind: "connected" };
        break;
      case "noSftp":
        phase = { kind: "noSftp", reason: event.reason };
        break;
      case "failed":
        // A typed login that did not get in is asked again next time.
        if (!typed) trying = null;
        hostKey = null;
        prompt = null;
        phase = { kind: "failed", reason: event.reason };
        break;
      case "lost":
        naming = null;
        confirming = null;
        phase = { kind: "lost", reason: event.reason };
        break;
    }
  }

  function show(next: Listing) {
    listing = next;
    path = next.folder;
  }

  function reconnect() {
    void start(listing?.folder ?? null);
  }

  function decide(decision: HostKeyDecision) {
    if (browser !== null) void decideFilesHostKey(browser, decision);
    hostKey = null;
  }

  function answer(answers: string[]) {
    if (browser !== null) void answerFiles(browser, answers);
    prompt = null;
  }

  function cancelPrompt() {
    if (browser !== null) void cancelFiles(browser);
    prompt = null;
  }

  function typedLogin([username, password]: string[]) {
    void open({ username: username ?? "", password: password ?? "" }, null);
  }

  /** Runs `action` (one at a time), showing its failure; a new listing it
   * gives is shown. */
  async function attempt(action: () => Promise<Listing>) {
    if (busy || browser === null) return;
    busy = true;
    error = null;
    try {
      show(await action());
      notice = null;
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }

  function go(to: string) {
    const id = browser;
    const from = listing?.folder ?? "";
    if (id !== null) void attempt(() => listFiles(id, from, to));
  }

  /** Runs `run` at once, or on a production host once confirmed. */
  function asking(text: string, label: string, run: () => void) {
    if (!production) return run();
    confirming = { text, label, run };
    void tick().then(() => cancelButton?.focus());
  }

  function startNaming(entry: RemoteEntry | null) {
    error = null;
    naming = { entry, value: entry?.name ?? "" };
    void tick().then(() => {
      nameInput?.focus();
      nameInput?.select();
    });
  }

  function submitName() {
    const current = naming;
    const id = browser;
    const folder = listing?.folder;
    if (!current || id === null || folder === undefined) return;
    naming = null;
    const value = current.value;
    const entry = current.entry;
    if (entry)
      asking(
        `Rename ${entry.name} to ${value.trim()} on ${name}?`,
        "Rename",
        () => void attempt(() => renameFile(id, folder, entry.name, value)),
      );
    else
      asking(
        `Make the folder ${value.trim()} on ${name}?`,
        "Make folder",
        () => void attempt(() => makeFolder(id, folder, value)),
      );
  }

  function remove(entry: RemoteEntry) {
    const id = browser;
    const folder = listing?.folder;
    if (id === null || folder === undefined) return;
    asking(
      `Delete ${entry.kind === "folder" ? `the folder ${entry.name}` : entry.name} on ${name}? It cannot be undone.`,
      "Delete",
      () => void attempt(() => deleteFile(id, folder, entry.name, entry.kind)),
    );
  }

  function escape(event: KeyboardEvent, close: () => void) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }

  const stateName = $derived(
    {
      starting: "connecting",
      credentials: "credentials",
      connecting: "connecting",
      connected: "connected",
      noSftp: "no-sftp",
      failed: "failed",
      lost: "lost",
    }[phase.kind],
  );
</script>

<div
  class="files"
  data-view="files"
  data-host={host}
  data-files-state={stateName}
  data-environment={production ? "production" : undefined}
>
  <header>
    <h2>Files on {name}</h2>
    {#if production}
      <span class="production" data-files-production>PROD</span>
    {/if}
    <span class="status" role="status">
      {#if phase.kind === "connecting" || phase.kind === "starting"}
        Connecting…
      {:else if phase.kind === "connected"}
        Connected
      {:else if phase.kind === "lost"}
        Connection lost
      {:else}
        Not connected
      {/if}
    </span>
  </header>

  {#if phase.kind === "connected" || (phase.kind === "lost" && listing)}
    <form
      class="bar"
      onsubmit={(event) => {
        event.preventDefault();
        go(path);
      }}
    >
      <button
        type="button"
        class="icon"
        data-files-action="up"
        title="Up to the folder above"
        disabled={busy || phase.kind !== "connected" || !listing?.parent}
        onclick={() => listing?.parent && go(listing.parent)}
        ><ArrowUp size={15} aria-hidden="true" /><span class="visually-hidden"
          >Up</span
        ></button
      >
      <input
        name="files-path"
        aria-label="Path"
        data-files-path
        bind:value={path}
        autocomplete="off"
        spellcheck="false"
        disabled={phase.kind !== "connected"}
      />
      <button
        type="submit"
        class="go"
        disabled={busy || phase.kind !== "connected"}>Go</button
      >
      <button
        type="button"
        class="icon"
        data-files-action="refresh"
        title="List this folder again"
        disabled={busy || phase.kind !== "connected"}
        onclick={() => go("")}
        ><RefreshCw size={15} aria-hidden="true" /><span class="visually-hidden"
          >Refresh</span
        ></button
      >
      <button
        type="button"
        class="icon"
        data-files-action="new-folder"
        title="Make a folder here"
        disabled={busy || phase.kind !== "connected"}
        onclick={() => startNaming(null)}
        ><FolderPlus size={15} aria-hidden="true" /><span
          class="visually-hidden">New folder</span
        ></button
      >
    </form>
  {/if}

  {#if phase.kind === "lost"}
    <div class="banner" role="alert" data-files-lost>
      <span data-files-reason>The connection was lost: {phase.reason}.</span>
      <button class="primary" data-files-action="reconnect" onclick={reconnect}
        >Reconnect</button
      >
    </div>
  {:else if phase.kind === "failed"}
    <div class="banner" role="alert" data-files-failed>
      <span data-files-reason>Not connected: {phase.reason}.</span>
      <button class="primary" data-files-action="reconnect" onclick={reconnect}
        >Connect again</button
      >
    </div>
  {:else if phase.kind === "noSftp"}
    <div class="banner" role="alert" data-files-no-sftp>
      <span
        >This device does not offer a listing of its files.
        <span class="muted" data-files-reason>({phase.reason})</span></span
      >
    </div>
  {/if}

  {#if notice}
    <p class="note" data-files-notice>{notice}</p>
  {/if}
  {#if error}
    <p class="error" role="alert" data-files-error>{error}</p>
  {/if}

  {#if listing && (phase.kind === "connected" || phase.kind === "lost")}
    <div class="table" data-files-folder={listing.folder}>
      <table>
        <thead>
          <tr>
            <th scope="col">Name</th>
            <th scope="col" class="number">Size</th>
            <th scope="col">Modified</th>
            <th scope="col">Permissions</th>
            <th scope="col"><span class="visually-hidden">Actions</span></th>
          </tr>
        </thead>
        <tbody>
          {#each listing.entries as entry (entry.name)}
            <tr data-files-entry={entry.name} data-kind={entry.kind}>
              <td class="name">
                {#if entry.kind === "folder" || entry.kind === "link"}
                  <button
                    class="link"
                    data-files-open={entry.name}
                    title={entry.kind === "link"
                      ? "A link: open what it points to"
                      : "Open this folder"}
                    disabled={busy || phase.kind !== "connected"}
                    onclick={() => go(entry.name)}
                  >
                    {#if entry.kind === "folder"}
                      <Folder size={14} aria-hidden="true" />
                    {:else}
                      <Link size={14} aria-hidden="true" />
                    {/if}
                    {entry.name}</button
                  >
                {:else}
                  <span class="plain"
                    ><FileIcon size={14} aria-hidden="true" />{entry.name}</span
                  >
                {/if}
              </td>
              <td class="number"
                >{entry.kind === "folder" ? "" : sizeText(entry.size)}</td
              >
              <td>{timeText(entry.modified)}</td>
              <td class="mono">{entry.permissions ?? ""}</td>
              <td class="actions">
                <div>
                  <button
                    data-files-entry-action="rename"
                    disabled={busy || phase.kind !== "connected"}
                    onclick={() => startNaming(entry)}>Rename</button
                  >
                  <button
                    class="danger"
                    data-files-entry-action="delete"
                    title={entry.kind === "folder"
                      ? "Delete this folder (it must be empty)"
                      : "Delete this file"}
                    disabled={busy || phase.kind !== "connected"}
                    onclick={() => remove(entry)}>Delete</button
                  >
                </div>
              </td>
            </tr>
          {:else}
            <tr
              ><td colspan="5" class="muted" data-files-empty
                >This folder is empty.</td
              ></tr
            >
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  {#if phase.kind === "credentials"}
    <div data-files-login>
      <LoginQuestions
        prompt={{
          ...LOGIN_PROMPT,
          name: `Log in to ${name}`,
          instructions: phase.why ?? "",
        }}
        ready={true}
        onanswer={typedLogin}
        oncancel={() =>
          (phase = { kind: "failed", reason: "no login was given" })}
      />
    </div>
  {/if}
  {#if hostKey}
    {#key hostKey.key}
      <HostKeyCheck
        question={hostKey.question}
        ready={browser !== null}
        ondecide={decide}
      />
    {/key}
  {/if}
  {#if prompt}
    {#key prompt.key}
      <LoginQuestions
        prompt={prompt.prompt}
        ready={browser !== null}
        onanswer={answer}
        oncancel={cancelPrompt}
      />
    {/key}
  {/if}
  {#if naming}
    {@const current = naming}
    <div
      class="backdrop"
      role="dialog"
      tabindex="-1"
      aria-label={current.entry ? `Rename ${current.entry.name}` : "New folder"}
      onkeydown={(event) => escape(event, () => (naming = null))}
    >
      <form
        class="card"
        data-files-name-form={current.entry ? "rename" : "new-folder"}
        onsubmit={(event) => {
          event.preventDefault();
          submitName();
        }}
      >
        <h3>{current.entry ? `Rename ${current.entry.name}` : "New folder"}</h3>
        <label>
          Name
          <input
            name="files-name"
            bind:this={nameInput}
            bind:value={current.value}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <div class="actions">
          <button
            type="button"
            data-files-name-action="cancel"
            onclick={() => (naming = null)}>Cancel</button
          >
          <button
            type="submit"
            data-files-name-action="save"
            disabled={!current.value.trim()}
            >{current.entry ? "Rename" : "Make folder"}</button
          >
        </div>
      </form>
    </div>
  {/if}
  {#if confirming}
    {@const question = confirming}
    <div
      class="backdrop"
      role="dialog"
      tabindex="-1"
      aria-labelledby="files-confirm-title"
      data-files-confirm
      data-environment="production"
      onkeydown={(event) => escape(event, () => (confirming = null))}
    >
      <div class="card">
        <h3 id="files-confirm-title">{question.text}</h3>
        <p>{name} is a production host.</p>
        <div class="actions">
          <button
            type="button"
            data-files-confirm-action="cancel"
            bind:this={cancelButton}
            onclick={() => (confirming = null)}>Cancel</button
          >
          <button
            type="button"
            class="danger"
            data-files-confirm-action="confirm"
            onclick={() => {
              // Read before clearing: `question` follows `confirming`.
              const run = question.run;
              confirming = null;
              run();
            }}>{question.label}</button
          >
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .files {
    position: relative;
    height: 100%;
    min-height: 0;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    overflow: hidden;
    background: var(--bg);
    font-size: var(--text-sm);
  }

  .files[data-environment="production"] {
    border-left: 3px solid var(--env-production);
  }

  header {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  h3 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  .production {
    padding: 0 var(--space-1);
    border-radius: var(--radius-sm);
    font-size: var(--text-2xs);
    font-weight: var(--weight-bold);
    color: var(--on-env-production);
    background: var(--env-production);
  }

  .status {
    margin-left: auto;
    color: var(--muted);
  }

  .bar {
    display: flex;
    gap: var(--space-1-5);
    align-items: center;
  }

  .bar input {
    flex: 1;
    min-width: 6rem;
    font-family: var(--font-mono);
  }

  .bar .go {
    padding: var(--space-1) var(--space-3);
  }

  button.icon {
    display: inline-grid;
    place-items: center;
    padding: var(--space-1);
  }

  .banner {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
    padding: var(--space-2);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    overflow-wrap: anywhere;
  }

  .note,
  .muted {
    margin: 0;
    color: var(--muted);
  }

  .error {
    margin: 0;
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .table {
    flex: 1;
    min-height: 0;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th {
    position: sticky;
    top: 0;
    text-align: left;
    font-weight: var(--weight-semibold);
    color: var(--muted);
    background: var(--surface);
  }

  th,
  td {
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }

  .number {
    text-align: right;
  }

  .mono {
    font-family: var(--font-mono);
  }

  td.name {
    width: 100%;
    max-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .name button.link,
  .name .plain {
    display: inline-flex;
    gap: var(--space-1);
    align-items: center;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  button.link {
    padding: 0;
    color: var(--accent);
    background: none;
    border: none;
    text-align: left;
  }

  td.actions > div {
    display: flex;
    gap: var(--space-1);
    justify-content: flex-end;
  }

  td.actions button {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  button.danger {
    color: var(--danger);
    border-color: var(--danger);
  }

  /* Over this tab only: the other panes stay usable (#74). */
  .backdrop {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--backdrop);
    overflow: auto;
  }

  .card {
    width: min(24rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  .card p {
    margin: 0;
  }

  .card label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .card .actions {
    flex-wrap: wrap;
  }
</style>
