<script lang="ts">
  // A host's files over SFTP, in its tab (#103; one tab per host, main
  // window only). It connects when it opens, with the host's stored profile
  // or a login typed here; host-key and login questions are asked over the
  // tab with the terminal's components. It lists one folder at a time and
  // renames, deletes and makes folders on the device; on a production host
  // each of those asks first, Cancel focused.
  //
  // Transfers (#106): Upload (a file dialog, or files dropped on the tab)
  // into the folder shown, Download of the selected files into a folder
  // picked (remembered for the next one). An upload asks first on a
  // production host (Cancel focused) and over files of the same name; a
  // download asks over local files of the same name. The transfer list
  // shows each one, waiting ones in order, finished ones kept while the tab
  // is open. A device without SFTP gets a form that sends and fetches by
  // path over SCP. The connection and the transfers end with the tab.
  //
  // Over SCP each transfer logs in again (#109): what that login asks (a
  // one-time code, a host key trusted once) comes as the browser's events
  // and is asked over the tab with the same components; Cancel fails that
  // transfer alone.
  import { onDestroy, onMount, tick } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import ArrowDown from "@lucide/svelte/icons/arrow-down";
  import ArrowUp from "@lucide/svelte/icons/arrow-up";
  import Download from "@lucide/svelte/icons/download";
  import FileIcon from "@lucide/svelte/icons/file";
  import Folder from "@lucide/svelte/icons/folder";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Link from "@lucide/svelte/icons/link";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Upload from "@lucide/svelte/icons/upload";
  import CopyButton from "./CopyButton.svelte";
  import HostKeyCheck from "./HostKeyCheck.svelte";
  import LoginQuestions from "./LoginQuestions.svelte";
  import { hostLogin } from "./credentials";
  import { setActiveTransfers } from "./fileTransfers.svelte";
  import {
    answerFiles,
    baseName,
    cancelFiles,
    cancelTransfer,
    closeFiles,
    closeFilesHost,
    decideFilesHostKey,
    deleteFile,
    downloadFiles,
    existingLocally,
    fetchFile,
    joinPath,
    listFiles,
    makeFolder,
    openFiles,
    presentFiles,
    renameFile,
    sendFile,
    sizeText,
    timeText,
    uploadFiles,
    type Added,
    type BrowserEvent,
    type Listing,
    type RemoteEntry,
    type TransferState,
    type TransferUpdate,
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
    /** No SFTP: transfers by path over SCP, no connection held. */
    | {
        kind: "transferOnly";
        reason: string;
        hint: string | null;
        folder: string | null;
      }
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
  /** The host ids browsers were opened for (one, unless it was renamed),
   * whose transfers end with the tab. */
  const opened = new Set<string>();

  let hostKey = $state<{ question: HostKeyQuestion; key: number } | null>(null);
  let prompt = $state<{ prompt: LoginPrompt; key: number } | null>(null);
  let questions = 0;

  let listing = $state<Listing | null>(null);
  let path = $state("");
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** The files of the listing chosen for Download, by name. */
  let selected = $state<string[]>([]);

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
  /** A transfer's question (production, or files of the same name),
   * waiting for its answer. */
  let transferQuestion = $state<{
    kind: "upload" | "download";
    title: string;
    /** The names that would be replaced. */
    replaced: string[];
    /** Where they are. */
    where: string;
    label: string;
    run: () => void;
  } | null>(null);
  let nameInput = $state<HTMLInputElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let transferCancel = $state<HTMLButtonElement>();
  let transferConfirm = $state<HTMLButtonElement>();
  let root = $state<HTMLDivElement>();

  /** One transfer in the list, with what its speed is worked out from. */
  interface Row {
    added: Added;
    state: TransferState;
    /** Bytes per second, smoothed; null until known. */
    speed: number | null;
    sample: { done: number; at: number } | null;
  }

  let transfers = $state<Row[]>([]);
  /** States that arrived before their transfer was listed. */
  const early = new Map<number, TransferState>();
  /** The folder the last download went into. */
  let downloadFolder = $state<string | null>(null);
  /** Files are being dragged over this tab. */
  let dragging = $state(false);

  /** The SCP form's fields. */
  let sendLocal = $state("");
  let sendRemote = $state("");
  /** The path the form filled in, until typed over. */
  let filledRemote = "";
  let fetchRemote = $state("");
  let fetchInto = $state("");

  const LOGIN_PROMPT: LoginPrompt = {
    name: "",
    instructions: "",
    questions: [
      { text: "Username", echo: true },
      { text: "Password", echo: false },
    ],
  };

  let unlistenDrops: (() => void) | null = null;
  let destroyed = false;
  let ticker: ReturnType<typeof setInterval> | undefined;

  onMount(() => {
    void start(null);
    // A transfer that moves no byte sends no state: its speed falls here.
    ticker = setInterval(() => {
      for (const row of transfers)
        if (row.state.kind === "running") sample(row, row.state.done);
    }, 1000);
    void getCurrentWebview()
      .onDragDropEvent((event) => ondrag(event.payload))
      .then((unlisten) => {
        if (destroyed) unlisten();
        else unlistenDrops = unlisten;
      });
  });
  onDestroy(() => {
    destroyed = true;
    clearInterval(ticker);
    unlistenDrops?.();
    generation += 1;
    // Closes the browsers too, once a cancelled upload removed its partial
    // file over their connection.
    for (const key of opened) void closeFilesHost(key);
  });

  const isActive = (state: TransferState) =>
    state.kind === "waiting" ||
    state.kind === "connecting" ||
    state.kind === "running";

  // The app asks before closing the tab, the window or the workspace
  // while any of these run or wait.
  $effect(() => {
    const counted = host;
    setActiveTransfers(
      counted,
      transfers.filter((row) => isActive(row.state)).length,
    );
    // A host renamed meanwhile leaves no count behind.
    return () => setActiveTransfers(counted, 0);
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

  /** Opens a new browser (closing the last), starting in `folder`. The
   * host's waiting transfers go on through it. */
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
    transferQuestion = null;
    error = null;
    phase = { kind: "connecting" };
    opened.add(host);
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
      case "transferOnly":
        typed = trying;
        if (!fetchInto && downloadFolder) fetchInto = downloadFolder;
        // The send's path starts as the platform's file folder (a chosen
        // file adds its name); a path typed by hand stays.
        if (
          event.folder &&
          (!sendRemote.trim() || sendRemote === filledRemote)
        ) {
          sendRemote = joinPath(event.folder, "");
          filledRemote = sendRemote;
        }
        phase = {
          kind: "transferOnly",
          reason: event.reason,
          hint: event.hint,
          folder: event.folder,
        };
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
        transferQuestion = null;
        phase = { kind: "lost", reason: event.reason };
        break;
      case "questionClosed":
        hostKey = null;
        prompt = null;
        break;
    }
  }

  function show(next: Listing) {
    if (next.folder !== listing?.folder) selected = [];
    else
      selected = selected.filter((name) =>
        next.entries.some((entry) => entry.name === name),
      );
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

  /** Lists the folder shown again after a transfer, leaving the path box
   * and the error as they are. */
  async function refreshListing() {
    const id = browser;
    const folder = listing?.folder;
    if (phase.kind !== "connected" || id === null || folder === undefined)
      return;
    try {
      const next = await listFiles(id, folder, "");
      if (browser === id && listing?.folder === folder) {
        const typedPath = path;
        show(next);
        path = typedPath;
      }
    } catch {
      // The next action says what is wrong.
    }
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

  // ---- Transfers ----

  const filesText = (names: string[]) =>
    names.length === 1 ? names[0] : `${names.length} files`;

  /** Asks the transfer question (Cancel focused on production for an
   * upload), or runs `run` when there is nothing to ask. */
  function askTransfer(question: NonNullable<typeof transferQuestion>) {
    const ask =
      question.replaced.length > 0 ||
      (question.kind === "upload" && production);
    if (!ask) return question.run();
    transferQuestion = question;
    void tick().then(() =>
      (question.kind === "upload" && production
        ? transferCancel
        : transferConfirm
      )?.focus(),
    );
  }

  function answerTransfer(go: boolean) {
    const question = transferQuestion;
    transferQuestion = null;
    if (go && question) question.run();
  }

  /** Lists transfers the core added, applying states that came first. */
  function list(added: Added[]) {
    for (const each of added) {
      const state = early.get(each.id) ?? { kind: "waiting" };
      early.delete(each.id);
      transfers.push({ added: each, state, speed: null, sample: null });
    }
  }

  function track(update: TransferUpdate) {
    const row = transfers.find((row) => row.added.id === update.id);
    if (!row) {
      early.set(update.id, update.state);
      return;
    }
    const was = row.state;
    const state = update.state;
    if (state.kind === "running") {
      if (was.kind !== "running") row.sample = null;
      sample(row, state.done);
    }
    row.state = state;
    if (removing.delete(row.added.id) && state.kind === "cancelled") {
      transfers.splice(transfers.indexOf(row), 1);
      return;
    }
    if (isActive(was) && !isActive(state)) void refreshListing();
  }

  /** Works out `row`'s speed from the bytes `done` now, at most every
   * 400 ms, smoothed. */
  function sample(row: Row, done: number) {
    const now = performance.now();
    if (row.sample === null) {
      row.sample = { done, at: now };
      return;
    }
    if (now - row.sample.at < 400) return;
    const rate = ((done - row.sample.done) * 1000) / (now - row.sample.at);
    row.speed = row.speed === null ? rate : row.speed * 0.6 + rate * 0.4;
    row.sample = { done, at: now };
  }

  /** Uploads `paths` into the folder shown, asking first on a production
   * host and over files of the same name there. */
  async function upload(paths: string[]) {
    const id = browser;
    const folder = listing?.folder;
    if (
      phase.kind !== "connected" ||
      id === null ||
      folder === undefined ||
      paths.length === 0
    )
      return;
    error = null;
    const names = paths.map(baseName);
    let replaced: string[];
    try {
      replaced = await presentFiles(id, folder, names);
    } catch (reason) {
      error = String(reason);
      return;
    }
    askTransfer({
      kind: "upload",
      title: `Upload ${filesText(names)} to ${folder} on ${name}?`,
      replaced,
      where: `in ${folder}`,
      label: replaced.length > 0 ? "Replace" : "Upload",
      run: () =>
        void startTransfers(() => uploadFiles(id, folder, paths, track)),
    });
  }

  async function startTransfers(add: () => Promise<Added[]>) {
    try {
      list(await add());
    } catch (reason) {
      error = String(reason);
    }
  }

  async function chooseUploads() {
    const picked = await openDialog({
      multiple: true,
      title: `Upload to ${listing?.folder ?? ""} on ${name}`,
    }).catch(() => null);
    if (picked && picked.length > 0) void upload(picked);
  }

  /** Asks for a local folder, starting in the last one. */
  async function chooseFolder(title: string): Promise<string | null> {
    const picked = await openDialog({
      directory: true,
      title,
      defaultPath: downloadFolder ?? undefined,
    }).catch(() => null);
    return typeof picked === "string" ? picked : null;
  }

  /** Downloads the selected files into a folder picked, asking only over
   * local files of the same name. */
  async function download() {
    const id = browser;
    const folder = listing?.folder;
    const names = [...selected];
    if (id === null || folder === undefined || names.length === 0) return;
    const into = await chooseFolder(`Download ${filesText(names)} into`);
    if (!into) return;
    downloadFolder = into;
    error = null;
    let replaced: string[];
    try {
      replaced = await existingLocally(into, names);
    } catch (reason) {
      error = String(reason);
      return;
    }
    askTransfer({
      kind: "download",
      title: `Download ${filesText(names)} into ${into}?`,
      replaced,
      where: `in ${into}`,
      label: "Replace",
      run: () =>
        void startTransfers(() =>
          downloadFiles(id, folder, names, into, track),
        ),
    });
  }

  function toggle(entry: RemoteEntry) {
    selected = selected.includes(entry.name)
      ? selected.filter((name) => name !== entry.name)
      : [...selected, entry.name];
  }

  /** Waiting transfers taken off the list: their row goes once the core
   * has them cancelled. */
  const removing = new Set<number>();

  function cancel(row: Row) {
    if (row.state.kind === "waiting") removing.add(row.added.id);
    void cancelTransfer(row.added.id);
  }

  /** A local file for the SCP form's send, with the path on the device
   * filled in: the platform's file folder and the file's name. */
  function chooseSendFile(local: string) {
    sendLocal = local;
    // A path typed by hand stays.
    if (sendRemote.trim() && sendRemote !== filledRemote) return;
    const folder = phase.kind === "transferOnly" ? phase.folder : null;
    const file = baseName(local.trim());
    sendRemote = folder ? joinPath(folder, file) : file;
    filledRemote = sendRemote;
  }

  async function pickSendFile() {
    const picked = await openDialog({
      title: `Send a file to ${name}`,
    }).catch(() => null);
    if (typeof picked === "string") chooseSendFile(picked);
  }

  async function pickFetchFolder() {
    const picked = await chooseFolder("Fetch into");
    if (picked) fetchInto = picked;
  }

  function send() {
    const id = browser;
    const local = sendLocal.trim();
    const remote = sendRemote.trim();
    if (id === null || !local || !remote) return;
    error = null;
    // The device cannot be asked whether the file is there.
    askTransfer({
      kind: "upload",
      title: `Send ${baseName(local)} to ${remote} on ${name}?`,
      replaced: [],
      where: "",
      label: "Send",
      run: () =>
        void startTransfers(async () => [
          await sendFile(id, local, remote, track),
        ]),
    });
  }

  async function fetchByPath() {
    const id = browser;
    const remote = fetchRemote.trim();
    const into = fetchInto.trim();
    if (id === null || !remote || !into) return;
    downloadFolder = into;
    error = null;
    const fileName = remote.split(/[/:]/).pop() ?? remote;
    let replaced: string[] = [];
    try {
      if (fileName) replaced = await existingLocally(into, [fileName]);
    } catch (reason) {
      error = String(reason);
      return;
    }
    askTransfer({
      kind: "download",
      title: `Fetch ${remote} into ${into}?`,
      replaced,
      where: `in ${into}`,
      label: "Replace",
      run: () =>
        void startTransfers(async () => [
          await fetchFile(id, remote, into, track),
        ]),
    });
  }

  // ---- Files dropped from Explorer ----

  type DragPayload =
    | { type: "enter"; paths: string[]; position: { x: number; y: number } }
    | { type: "over"; position: { x: number; y: number } }
    | { type: "drop"; paths: string[]; position: { x: number; y: number } }
    | { type: "leave" };

  /** Whether the drop point (in physical pixels) is on this tab, shown
   * and with no question over it. */
  function over(position: { x: number; y: number }): boolean {
    if (!root) return false;
    const ratio = window.devicePixelRatio || 1;
    const element = document.elementFromPoint(
      position.x / ratio,
      position.y / ratio,
    );
    return (
      !!element &&
      root.contains(element) &&
      !root.querySelector(".backdrop, [data-files-login]")
    );
  }

  function ondrag(payload: DragPayload) {
    const accepts = phase.kind === "connected" || phase.kind === "transferOnly";
    switch (payload.type) {
      case "enter":
      case "over":
        dragging = accepts && over(payload.position);
        break;
      case "drop":
        dragging = false;
        if (!accepts || !over(payload.position)) return;
        if (phase.kind === "connected") void upload(payload.paths);
        else if (payload.paths[0]) chooseSendFile(payload.paths[0]);
        break;
      case "leave":
        dragging = false;
        break;
    }
  }

  // ---- Showing transfers ----

  function waitingBefore(row: Row): number {
    let before = 0;
    for (const other of transfers) {
      if (other === row) break;
      if (isActive(other.state)) before += 1;
    }
    return before;
  }

  /** The core's words as a sentence. */
  const sentence = (text: string) =>
    `${text.charAt(0).toUpperCase()}${text.slice(1)}.`;

  function percent(state: TransferState): number | null {
    if (state.kind !== "running" || !state.total) return null;
    return Math.min(100, Math.round((state.done * 100) / state.total));
  }

  const stateName = $derived(
    {
      starting: "connecting",
      credentials: "credentials",
      connecting: "connecting",
      connected: "connected",
      transferOnly: "transfer-only",
      failed: "failed",
      lost: "lost",
    }[phase.kind],
  );

  const files = $derived(
    (listing?.entries ?? []).filter((entry) => entry.kind === "file"),
  );
</script>

<div
  class="files"
  class:dragging
  bind:this={root}
  data-view="files"
  data-host={host}
  data-files-state={stateName}
  data-files-dragging={dragging || undefined}
  data-environment={production ? "production" : undefined}
>
  <header>
    <h2>Files on {name}</h2>
    {#if production}
      <span class="production" data-files-production>PROD</span>
    {/if}
    <span class="status" role="status" data-files-status>
      {#if phase.kind === "connecting" || phase.kind === "starting"}
        Connecting…
      {:else if phase.kind === "connected"}
        Connected
      {:else if phase.kind === "transferOnly"}
        No connection held: one is made for each transfer
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
      <button
        type="button"
        class="text"
        data-files-action="upload"
        title="Upload files into this folder (or drop them on the tab)"
        disabled={phase.kind !== "connected"}
        onclick={chooseUploads}
        ><Upload size={15} aria-hidden="true" />Upload</button
      >
      <button
        type="button"
        class="text"
        data-files-action="download"
        title={selected.length > 0
          ? "Download the selected files into a folder"
          : "Select files to download"}
        disabled={phase.kind !== "connected" || selected.length === 0}
        onclick={download}
        ><Download size={15} aria-hidden="true" />Download{selected.length > 1
          ? ` (${selected.length})`
          : ""}</button
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
  {/if}

  {#if notice}
    <p class="note" data-files-notice>{notice}</p>
  {/if}
  {#if error}
    <p class="error" role="alert" data-files-error>{error}</p>
  {/if}

  {#if phase.kind === "transferOnly"}
    <div class="scp" data-files-transfer-only>
      <div class="banner">
        <span
          >This device cannot list its files; it can send and fetch a file by
          its path.
          <span class="muted" data-files-reason>({phase.reason})</span></span
        >
      </div>
      {#if phase.hint}
        <p class="note" data-files-hint>{phase.hint}</p>
      {/if}
      <form
        class="scp-form"
        data-files-send
        onsubmit={(event) => {
          event.preventDefault();
          send();
        }}
      >
        <h3>Send a file</h3>
        <label>
          Local file
          <span class="row">
            <input
              name="files-send-local"
              bind:value={sendLocal}
              autocomplete="off"
              spellcheck="false"
              placeholder="Choose a file, or drop one on the tab"
              oninput={() => chooseSendFile(sendLocal)}
            />
            <button
              type="button"
              data-files-action="choose-send"
              onclick={pickSendFile}>Choose…</button
            >
          </span>
        </label>
        <label>
          Path on the device
          <input
            name="files-send-remote"
            bind:value={sendRemote}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <div class="actions">
          <button
            type="submit"
            data-files-action="send"
            disabled={!sendLocal.trim() || !sendRemote.trim()}>Send</button
          >
        </div>
      </form>
      <form
        class="scp-form"
        data-files-fetch
        onsubmit={(event) => {
          event.preventDefault();
          void fetchByPath();
        }}
      >
        <h3>Fetch a file</h3>
        <label>
          Path on the device
          <input
            name="files-fetch-remote"
            bind:value={fetchRemote}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <label>
          Local folder
          <span class="row">
            <input
              name="files-fetch-local"
              bind:value={fetchInto}
              autocomplete="off"
              spellcheck="false"
            />
            <button
              type="button"
              data-files-action="choose-fetch"
              onclick={pickFetchFolder}>Choose…</button
            >
          </span>
        </label>
        <div class="actions">
          <button
            type="submit"
            data-files-action="fetch"
            disabled={!fetchRemote.trim() || !fetchInto.trim()}>Fetch</button
          >
        </div>
      </form>
      <p class="note" data-files-replaced>
        A file of that name on the device is replaced: this device cannot be
        asked whether one is there.
      </p>
    </div>
  {/if}

  {#if listing && (phase.kind === "connected" || phase.kind === "lost")}
    <div class="table" data-files-folder={listing.folder}>
      <table>
        <thead>
          <tr>
            <th scope="col" class="select"
              ><span class="visually-hidden">Selected</span></th
            >
            <th scope="col">Name</th>
            <th scope="col" class="number">Size</th>
            <th scope="col">Modified</th>
            <th scope="col">Permissions</th>
            <th scope="col"><span class="visually-hidden">Actions</span></th>
          </tr>
        </thead>
        <tbody>
          {#each listing.entries as entry (entry.name)}
            <tr
              data-files-entry={entry.name}
              data-kind={entry.kind}
              data-selected={selected.includes(entry.name) || undefined}
            >
              <td class="select">
                {#if entry.kind === "file"}
                  <input
                    type="checkbox"
                    data-files-select={entry.name}
                    aria-label="Select {entry.name}"
                    checked={selected.includes(entry.name)}
                    disabled={phase.kind !== "connected"}
                    onchange={() => toggle(entry)}
                  />
                {/if}
              </td>
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
              ><td colspan="6" class="muted" data-files-empty
                >This folder is empty.</td
              ></tr
            >
          {/each}
        </tbody>
      </table>
    </div>
    {#if phase.kind === "connected" && files.length > 0 && selected.length === 0}
      <p class="note hint">
        Tick files to download them; drop files on the tab to upload them here.
      </p>
    {/if}
  {/if}

  {#if transfers.length > 0}
    <section class="transfers" aria-label="Transfers" data-files-transfers>
      <h3>Transfers</h3>
      <ol>
        {#each transfers as row (row.added.id)}
          {@const state = row.state}
          <li
            data-transfer={row.added.id}
            data-transfer-name={row.added.name}
            data-direction={row.added.direction}
            data-transfer-state={state.kind}
          >
            <div class="line">
              <span
                class="direction"
                title={row.added.direction === "upload" ? "Upload" : "Download"}
              >
                {#if row.added.direction === "upload"}
                  <ArrowUp size={14} aria-hidden="true" />
                {:else}
                  <ArrowDown size={14} aria-hidden="true" />
                {/if}
                <span class="visually-hidden"
                  >{row.added.direction === "upload"
                    ? "Upload"
                    : "Download"}</span
                >
              </span>
              <span
                class="transfer-name"
                title={row.added.direction === "upload"
                  ? `${row.added.local} → ${row.added.remote}`
                  : `${row.added.remote} → ${row.added.local}`}
                >{row.added.name}</span
              >
              <span class="what" data-transfer-progress>
                {#if state.kind === "waiting"}
                  {@const before = waitingBefore(row)}
                  Waiting{before > 0 ? ` (after ${before})` : ""}
                {:else if state.kind === "connecting"}
                  Connecting…
                {:else if state.kind === "running"}
                  {sizeText(state.done)}{state.total !== null
                    ? ` of ${sizeText(state.total)}`
                    : ""}
                {:else if state.kind === "done"}
                  Done, {sizeText(state.bytes)}
                {:else if state.kind === "failed"}
                  Failed
                {:else}
                  Cancelled
                {/if}
              </span>
              {#if state.kind === "running"}
                <span class="speed" data-transfer-speed
                  >{row.speed === null
                    ? ""
                    : `${sizeText(Math.round(row.speed))}/s`}</span
                >
              {/if}
              {#if isActive(state)}
                <button
                  type="button"
                  class="small"
                  data-transfer-action="cancel"
                  title={state.kind === "waiting"
                    ? "Take it off the list"
                    : "Stop this transfer"}
                  onclick={() => cancel(row)}
                  >{state.kind === "waiting" ? "Remove" : "Cancel"}</button
                >
              {/if}
            </div>
            {#if state.kind === "running"}
              {@const done = percent(state)}
              <div
                class="bar-track"
                role="progressbar"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={done ?? undefined}
              >
                <div class="bar-fill" style:width="{done ?? 0}%"></div>
              </div>
            {:else if state.kind === "done"}
              <p class="outcome" data-transfer-outcome>
                {state.checked ? "The size was checked." : sentence(state.size)}
              </p>
              {#if state.hashes}
                <dl class="hashes">
                  <dt>MD5</dt>
                  <dd data-transfer-hash="md5">
                    <code>{state.hashes.md5}</code>
                    <CopyButton value={state.hashes.md5} what="the MD5" />
                  </dd>
                  <dt>SHA-256</dt>
                  <dd data-transfer-hash="sha256">
                    <code>{state.hashes.sha256}</code>
                    <CopyButton
                      value={state.hashes.sha256}
                      what="the SHA-256"
                    />
                  </dd>
                </dl>
              {/if}
            {:else if state.kind === "failed"}
              <p class="outcome error" data-transfer-outcome>
                <span data-transfer-reason>{state.reason}</span>
              </p>
              {#if state.hint}
                <p class="outcome note" data-transfer-hint>{state.hint}</p>
              {/if}
              <p class="outcome note" data-transfer-partial>
                {sentence(state.partial)}
              </p>
            {:else if state.kind === "cancelled"}
              <p class="outcome note" data-transfer-outcome>
                <span data-transfer-partial>{sentence(state.partial)}</span>
              </p>
            {/if}
          </li>
        {/each}
      </ol>
    </section>
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
  {#if transferQuestion}
    {@const question = transferQuestion}
    {@const asksProduction = question.kind === "upload" && production}
    <div
      class="backdrop"
      role="dialog"
      tabindex="-1"
      aria-labelledby="files-transfer-title"
      data-files-transfer-confirm={question.kind}
      data-files-replace={question.replaced.length}
      data-environment={asksProduction ? "production" : undefined}
      onkeydown={(event) => escape(event, () => answerTransfer(false))}
    >
      <div class="card">
        <h3 id="files-transfer-title">{question.title}</h3>
        {#if asksProduction}
          <p data-files-transfer-production>{name} is a production host.</p>
        {/if}
        {#if question.replaced.length > 0}
          <p>
            {question.replaced.length === 1
              ? "A file of this name is already there"
              : "Files of these names are already there"}
            {question.where}; {question.kind === "upload"
              ? "uploading"
              : "downloading"} replaces {question.replaced.length === 1
              ? "it"
              : "them"}:
          </p>
          <ul class="replaced">
            {#each question.replaced as replaced (replaced)}
              <li data-files-replace-name={replaced}>{replaced}</li>
            {/each}
          </ul>
        {/if}
        <div class="actions">
          <button
            type="button"
            data-files-transfer-action="cancel"
            bind:this={transferCancel}
            onclick={() => answerTransfer(false)}>Cancel</button
          >
          <button
            type="button"
            class={asksProduction ? "danger" : ""}
            data-files-transfer-action="confirm"
            bind:this={transferConfirm}
            onclick={() => answerTransfer(true)}>{question.label}</button
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

  .files.dragging {
    outline: 2px dashed var(--accent);
    outline-offset: -4px;
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
    text-align: right;
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

  button.text {
    display: inline-flex;
    gap: var(--space-1);
    align-items: center;
    padding: var(--space-1) var(--space-2);
    white-space: nowrap;
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

  .hint {
    font-size: var(--text-xs);
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

  th.select,
  td.select {
    width: 1.5rem;
    padding-right: 0;
  }

  tr[data-selected] td {
    background: var(--surface);
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

  td.actions button,
  button.small {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  button.danger {
    color: var(--danger);
    border-color: var(--danger);
  }

  .scp {
    display: grid;
    gap: var(--space-2);
    overflow: auto;
    min-height: 0;
  }

  .scp-form {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }

  .scp-form label,
  .card label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .scp-form input {
    font-family: var(--font-mono);
  }

  .row {
    display: flex;
    gap: var(--space-1-5);
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .transfers {
    flex: none;
    max-height: 40%;
    display: grid;
    grid-template-rows: auto 1fr;
    gap: var(--space-1);
    min-height: 0;
  }

  .transfers ol {
    margin: 0;
    padding: 0;
    list-style: none;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .transfers li {
    display: grid;
    gap: var(--space-1);
    padding: var(--space-1-5) var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  .transfers li:last-child {
    border-bottom: none;
  }

  .line {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }

  .direction {
    display: inline-grid;
    place-items: center;
    color: var(--muted);
  }

  .transfer-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
  }

  .what,
  .speed {
    color: var(--muted);
    white-space: nowrap;
  }

  .bar-track {
    height: 4px;
    background: var(--surface);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .bar-fill {
    height: 100%;
    background: var(--accent);
  }

  .outcome {
    margin: 0;
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  .hashes {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--space-0-5) var(--space-2);
    margin: 0;
    font-size: var(--text-xs);
  }

  .hashes dt {
    color: var(--muted);
  }

  .hashes dd {
    display: flex;
    gap: var(--space-1);
    align-items: center;
    margin: 0;
    min-width: 0;
  }

  .hashes code {
    overflow-wrap: anywhere;
    font-family: var(--font-mono);
  }

  .replaced {
    margin: 0;
    padding-left: var(--space-4);
    max-height: 10rem;
    overflow: auto;
    font-family: var(--font-mono);
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

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .card .actions {
    flex-wrap: wrap;
  }
</style>
