// The interface's view of the core's workspace: the tree it loaded, the
// files it could not load, and the edits it can make. Everything here is a
// call into the Tauri shell; the core writes the files and returns the
// reloaded workspace.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { JobStep, ListedSavedJob, SavedJob } from "./jobs";

export interface CustomField {
  key: string;
  value: string;
}

export type Protocol = "ssh" | "telnet" | "serial";
export type Environment = "production" | "staging" | "lab";
export type Parity = "none" | "odd" | "even" | "mark" | "space";
export type FlowControl = "none" | "hardware" | "software";

/**
 * What applies to pastes into a host's sessions, and the environment that
 * colours them; see `netterm_core::paste`.
 */
export interface PasteSafety {
  environment: Environment | null;
  /** Milliseconds to wait between pasted lines. */
  lineDelayMs: number;
}

/** The core's limit on `paste_line_delay`, in milliseconds. */
export const MAX_PASTE_LINE_DELAY_MS = 10_000;

export interface WebLink {
  name: string;
  url: string;
}

export interface SerialSettings {
  /** The serial port to open, such as `COM3`. */
  port: string | null;
  baud_rate: number | null;
  data_bits: number | null;
  parity: Parity | null;
  stop_bits: number | null;
  flow_control: FlowControl | null;
}

/**
 * The connection settings a host or folder sets itself; null (or an empty
 * list) inherits from the folders above.
 */
export interface Settings {
  protocol: Protocol | null;
  port: number | null;
  credential_profile: string | null;
  /** The id of another host to connect through. */
  jump_host: string | null;
  legacy_crypto: boolean | null;
  /** Seconds opening a session may wait on the device. */
  connect_timeout: number | null;
  logging: boolean | null;
  /** Whether SSH sessions save the configuration once connected. */
  backup_on_login: boolean | null;
  environment: Environment | null;
  /** Milliseconds between pasted lines (0 to `MAX_PASTE_LINE_DELAY_MS`). */
  paste_line_delay: number | null;
  /** The kind of device (`eos`, `srlinux`, ...); any text. */
  platform: string | null;
  web_links: WebLink[];
  serial: SerialSettings;
}

/** Where an effective setting comes from. */
export type Origin =
  { kind: "default" } | { kind: "host" } | { kind: "folder"; path: string };

export interface Effective<T> {
  value: T;
  origin: Origin;
}

/** What applies to a host or folder once inheritance is resolved. */
export interface EffectiveSettings {
  protocol: Effective<Protocol>;
  /** Null: the protocol's own default port. */
  port: Effective<number> | null;
  credential_profile: Effective<string> | null;
  jump_host: Effective<string> | null;
  legacy_crypto: Effective<boolean>;
  /** In seconds. */
  connect_timeout: Effective<number>;
  logging: Effective<boolean>;
  /** Off by default. */
  backup_on_login: Effective<boolean>;
  environment: Effective<Environment> | null;
  /** In milliseconds; 0 by default. */
  paste_line_delay: Effective<number>;
  platform: Effective<string> | null;
  web_links: Effective<WebLink>[];
  serial: {
    /** Null when nothing sets a port. */
    port: Effective<string> | null;
    baud_rate: Effective<number>;
    data_bits: Effective<number>;
    parity: Effective<Parity>;
    stop_bits: Effective<number>;
    flow_control: Effective<FlowControl>;
  };
  /** Every tag from the root down, with its nearest origin. */
  tags: Effective<string>[];
}

export interface Host {
  /** The host file's path in the workspace; identifies the host. */
  id: string;
  name: string;
  /** The management address. */
  address: string;
  vendor: string | null;
  model: string | null;
  serial_number: string | null;
  site: string | null;
  building: string | null;
  room: string | null;
  rack: string | null;
  rack_unit: string | null;
  /** The id of the host's role, as its file names it. */
  role: string | null;
  description: string | null;
  tags: string[];
  custom: CustomField[];
  /** The settings the host sets itself (its own platform among them). */
  settings: Settings;
}

/** The optional built-in text fields of a host, in display order. */
export const HOST_TEXT_FIELDS = [
  ["vendor", "Vendor"],
  ["model", "Model"],
  ["serial_number", "Serial number"],
  ["site", "Site"],
  ["building", "Building"],
  ["room", "Room"],
  ["rack", "Rack"],
  ["rack_unit", "Rack unit"],
] as const satisfies readonly (readonly [keyof Host, string])[];

export interface Folder {
  name: string;
  /** Path relative to the workspace root; empty for the root. */
  path: string;
  description: string | null;
  /** Tags every host below carries too. */
  tags: string[];
  /** The settings the folder sets for everything below it. */
  settings: Settings;
  folders: Folder[];
  hosts: Host[];
}

/** The icons a role can have; see `RoleIcon.svelte`. */
export type RoleIcon =
  | "core"
  | "distribution"
  | "access"
  | "router"
  | "firewall"
  | "load_balancer"
  | "wireless_controller"
  | "access_point"
  | "console_server"
  | "server"
  | "switch"
  | "cloud"
  | "storage"
  | "generic"
  | "unknown";

/** A role a host can have: built in, the workspace's own, or undefined. */
export interface Role {
  id: string;
  name: string;
  icon: RoleIcon;
  /** `#rrggbb`. */
  color: string;
  source: "built_in" | "workspace" | "undefined";
}

export interface Problem {
  file: string;
  message: string;
}

/**
 * A condition of a saved filter. `field` is a built-in host field (`name`,
 * `address`, `vendor`, ..., `role`), `tag`, `custom.<key>` or `text`;
 * `value` matches the whole value ignoring case, `*` standing for any text
 * (for `text`: words searched as the palette does).
 */
export interface Condition {
  field: string;
  value: string;
}

/** A named view of the inventory: the hosts meeting every condition. */
export interface SavedFilter {
  name: string;
  conditions: Condition[];
}

/** What a saved filter shows: its hosts, and the folders holding them. */
export interface FilterView {
  hosts: string[];
  folders: string[];
}

/** A field of a host that a search matched. */
export interface FieldMatch {
  /** `name`, `address`, `tag`, a built-in field, `role` or `custom.<key>`. */
  field: string;
  value: string;
}

export interface SearchHit {
  id: string;
  name: string;
  address: string;
  matches: FieldMatch[];
}

export interface SearchResults {
  /** The best hits, best first. */
  hits: SearchHit[];
  /** How many hosts matched in all. */
  total: number;
}

/**
 * A highlight rule: text matching `pattern` (a regular expression of the
 * core's syntax; `(?i)` ignores case) shows in `color`.
 */
export interface HighlightRule {
  name: string;
  pattern: string;
  /** `#rrggbb`, or a terminal palette colour (`HIGHLIGHT_COLOR_NAMES`). */
  color: string;
  enabled: boolean;
}

/** The palette colours a rule can use; they follow the terminal's theme. */
export const HIGHLIGHT_COLOR_NAMES = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "bright-black",
  "bright-red",
  "bright-green",
  "bright-yellow",
  "bright-blue",
  "bright-magenta",
  "bright-cyan",
  "bright-white",
] as const;

/** A rule as the workspace lists it. */
export interface ListedRule {
  rule: HighlightRule;
  /** `changed`: a built-in rule the workspace changes. */
  source: "builtIn" | "changed" | "workspace";
  /** Why its pattern cannot be used; such a rule highlights nothing. */
  error: string | null;
}

/** A highlighted part of a line, in code points from its start. */
export interface HighlightSpan {
  start: number;
  /** Exclusive. */
  end: number;
  color: string;
  rule: string;
}

/**
 * How the app drives a kind of device. Patterns (`prompt`,
 * `privilege_prompt`, `ignored_lines`) are regular expressions of the
 * core's syntax; null values are not set.
 */
export interface PlatformDefinition {
  name: string;
  /** Other names a host's platform may give for it (e.g. `arista_eos`). */
  otherNames: string[];
  prompt: string;
  disablePaging: string | null;
  privilegeCommand: string | null;
  privilegePrompt: string | null;
  showConfig: string | null;
  ignoredLines: string | null;
  /** Where files belong on the device, as it writes the path (`flash:`). */
  fileFolder: string | null;
}

/** A platform definition as the workspace lists it. */
export interface ListedPlatform {
  definition: PlatformDefinition;
  /** `changed`: a built-in definition the workspace changes. */
  source: "builtIn" | "changed" | "workspace";
  /** Why a pattern cannot be used; such a definition is not used. */
  error: string | null;
}

/**
 * A named piece of text to send to a session. Its text is a template:
 * `{{address}}`, `{{name}}`, `{{custom.<key>}}` and the host's other fields
 * are filled in; the user is asked for any other `{{variable}}`.
 */
export interface Snippet {
  name: string;
  /** What the panel lists it under; null: no group. */
  group: string | null;
  text: string;
}

/** A snippet made ready to send: its lines, or the variables to ask for. */
export type Expansion =
  { kind: "lines"; lines: string[] } | { kind: "missing"; names: string[] };

export interface WorkspaceView {
  /** Higher is newer: a view older than the one shown is stale. */
  revision: number;
  root: string;
  tree: Folder;
  /** The roles a host can be given. */
  roles: Role[];
  /** The role each host that has one shows (neutral if undefined), by id. */
  host_roles: Record<string, Role>;
  /** Every host's environment and paste line delay, by id. */
  host_safety: Record<string, PasteSafety>;
  /** The saved filters, in file order. */
  filters: SavedFilter[];
  /** The highlight rules: the workspace's own first, then the built-in. */
  highlight_rules: ListedRule[];
  /** The platform definitions: the built-in ones first. */
  platforms: ListedPlatform[];
  /**
   * Each usable definition's name and other names (trimmed, lower case)
   * with the name of the definition it finds.
   */
  platform_names: Record<string, { definition: string; usable: boolean }>;
  /** The snippets, in file order. */
  snippets: Snippet[];
  /** The saved jobs, in file order. */
  saved_jobs: ListedSavedJob[];
  problems: Problem[];
  /**
   * Why what is shown is not what the folder holds now: it cannot be
   * reached, or came back as another workspace. Edits are refused
   * meanwhile; null when the workspace is loaded from its folder.
   */
  unavailable: Unavailable | null;
}

/** Why the workspace shown is not what its folder holds now. */
export type Unavailable = (
  | { kind: "unreachable"; reason: string }
  | { kind: "otherWorkspace"; id: string | null }
) & {
  /** What it is, as one line. */
  message: string;
  /** What happens meanwhile and what to do. */
  explanation: string;
};

/** The workspace after an edit, and what the edit returned. */
export interface Edited<T> {
  workspace: WorkspaceView;
  result: T;
}

/** A host whose id changed: renamed or moved, or a folder above it was. */
export interface HostMove {
  from: string;
  to: string;
}

/** History that did not follow its host to the new path (#99). */
export interface LeftHistory {
  /** The host's new id, and its id before. */
  host: string;
  from: string;
  kind: "sessionLogs" | "configHistory";
  /** Where the history (or the part not moved) is. */
  folder: string;
  reason: string;
}

/**
 * The workspace after renaming or moving a host or folder: the new id or
 * path, the hosts that moved, the references to them that could not be
 * rewritten and the history that stayed behind.
 */
export interface Relocated<T> extends Edited<T> {
  moved: HostMove[];
  notRewritten: Problem[];
  left: LeftHistory[];
}

export type FolderDeleted =
  | { kind: "deleted"; workspace: WorkspaceView }
  | { kind: "needsConfirmation"; hosts: number };

/** Asks the user for a folder (titled `title`); null if they cancel. */
export async function pickWorkspaceFolder(
  title = "Open workspace folder",
): Promise<string | null> {
  const folder = await open({ directory: true, title });
  return typeof folder === "string" ? folder : null;
}

/**
 * What a folder holds, as the core tells (#131): host files or a
 * workspace's own files, nothing (hidden entries and empty folders), or
 * other things only, which opening would make a workspace.
 */
export type FolderKind = "workspace" | "empty" | "other";

/**
 * What the start found about where the app's data is (#128: config
 * backups, job results and the Python environment moving to the local
 * data folder) that the user should be told.
 */
export interface DataNotice {
  what: "configs" | "jobs" | "python";
  kind: "notMoved" | "bothHold" | "otherCopyRunning" | "leftBehind";
  message: string;
}

/** The start's notices about the data folders; empty when none. */
export function dataNotices(): Promise<DataNotice[]> {
  return invoke<DataNotice[]>("data_notices");
}

/** What kind of folder `path` is; nothing is written. */
export function folderKind(path: string): Promise<FolderKind> {
  return invoke<FolderKind>("workspace_folder_kind", { path });
}

/**
 * Opens a workspace folder (an existing one is added to the known
 * workspaces); it reopens on the next launch. A folder of other things is
 * refused unless the user said yes to making it a workspace (`confirmed`).
 */
export function openWorkspace(
  path: string,
  confirmed = false,
): Promise<WorkspaceView> {
  return invoke<WorkspaceView>("workspace_open", { path, confirmed });
}

/**
 * Creates a new, empty workspace in `path` (a missing or empty folder) and
 * opens it.
 */
export function createWorkspace(path: string): Promise<WorkspaceView> {
  return invoke<WorkspaceView>("workspace_create", { path });
}

/** A workspace the app knows, as the switcher lists it. */
export interface KnownWorkspace {
  path: string;
  /** Its folder's name. */
  name: string;
  /** Whether it is the open workspace. */
  open: boolean;
  /** Whether its folder is there. */
  available: boolean;
}

/** The known workspaces, most recently opened first. */
export function listWorkspaces(): Promise<KnownWorkspace[]> {
  return invoke<KnownWorkspace[]>("workspaces_list");
}

/** Takes a workspace off the list (its folder is left alone). */
export function forgetWorkspace(path: string): Promise<KnownWorkspace[]> {
  return invoke<KnownWorkspace[]>("workspace_forget", { path });
}

/**
 * Calls `handler` with the open workspace each time files changed outside
 * the app (a git pull, an editor) and the core reloaded it.
 */
export function onWorkspaceChanged(
  handler: (workspace: WorkspaceView) => void,
): Promise<UnlistenFn> {
  return listen<WorkspaceView>("workspace-changed", (event) =>
    handler(event.payload),
  );
}

/** The workspace open now, if any (a session window's palette, #88). */
export function currentWorkspace(): Promise<WorkspaceView | null> {
  return invoke<WorkspaceView | null>("workspace_current");
}

/** Reopens the last opened workspace, if any. */
export function restoreWorkspace(): Promise<WorkspaceView | null> {
  return invoke<WorkspaceView | null>("workspace_restore");
}

/** One of a host's web links, filled and checked by the core. */
export interface HostWebLink {
  name: string;
  /** The URL as written, with its variables. */
  template: string;
  origin: Origin;
  /** What clicking it opens; `null` if it is refused. */
  url: string | null;
  /** Why it is refused (not an http/https URL, a field without a value). */
  refused: string | null;
}

/** The web links that apply to the host `id`, merged down the tree. */
export function hostWebLinks(id: string): Promise<HostWebLink[]> {
  return invoke("host_web_links", { id });
}

/**
 * Opens the host `id`'s web link `name` in the default browser and returns
 * the URL; rejected with the reason unless it is an http or https URL.
 */
export function openWebLink(id: string, name: string): Promise<string> {
  return invoke("web_link_open", { id, name });
}

/**
 * Shows the folder of the host's session logs in the system's file manager
 * (created first if nothing was logged yet).
 */
export function openLogFolder(id: string): Promise<void> {
  return invoke("logs_open_folder", { id });
}

/** Returns the new folder's path. */
export function createFolder(parent: string, name: string) {
  return invoke<Edited<string>>("folder_create", { parent, name });
}

/** Returns the folder's new path. */
export function renameFolder(path: string, name: string) {
  return invoke<Relocated<string>>("folder_rename", { path, name });
}

/** Returns the folder's new path. */
export function moveFolder(path: string, parent: string) {
  return invoke<Relocated<string>>("folder_move", { path, parent });
}

/**
 * Deletes a folder. Unless `includingHosts`, a folder holding hosts is not
 * deleted, and the answer says how many hosts it holds.
 */
export function deleteFolder(path: string, includingHosts: boolean) {
  return invoke<FolderDeleted>("folder_delete", {
    path,
    deletion: includingHosts ? "includingHosts" : "ifNoHosts",
  });
}

export function setFolderDescription(path: string, description: string) {
  return invoke<Edited<null>>("folder_set_description", {
    path,
    description: description.trim() === "" ? null : description,
  });
}

/** Saves the settings a folder sets for everything below it. */
export function saveFolderSettings(path: string, settings: Settings) {
  return invoke<Edited<null>>("folder_save_settings", { path, settings });
}

export function setFolderTags(path: string, tags: string[]) {
  return invoke<Edited<null>>("folder_set_tags", { path, tags });
}

/** The settings that apply to a host or folder, with their origins. */
export function resolveSettings(
  item: { kind: "host"; id: string } | { kind: "folder"; path: string },
) {
  return invoke<EffectiveSettings>("settings_resolve", { item });
}

/** Returns the new host's id. */
export function createHost(folder: string, name: string, address: string) {
  return invoke<Edited<string>>("host_create", { folder, name, address });
}

// --- CSV import (see `netterm_core::import::csv`) ---

/** The built-in host fields a CSV column can fill, by file key. */
export type HostField =
  | "name"
  | "address"
  | "vendor"
  | "platform"
  | "model"
  | "serial_number"
  | "site"
  | "building"
  | "room"
  | "rack"
  | "rack_unit"
  | "role"
  | "description";

/** What a CSV column fills. */
export type ColumnTarget =
  | { kind: "field"; field: HostField }
  | { kind: "custom"; key: string }
  | { kind: "tags" }
  | { kind: "folderPath" }
  | { kind: "ignore" };

/** A CSV file as read, before mapping. */
export interface CsvFileView {
  separator: "comma" | "semicolon";
  headers: string[];
  /** How many rows there are below the header. */
  rows: number;
  /** The first rows' values. */
  sample: string[][];
  /** The core's guess at a mapping, one per column. */
  suggested: ColumnTarget[];
}

export interface ReportEntry {
  /** The line in the file; null for the whole file (a password column). */
  line: number | null;
  name: string;
  host: string | null;
  reason: string | null;
}

export type PreviewOutcome =
  | { kind: "create"; id: string }
  | { kind: "exists"; id: string }
  | { kind: "skip"; reason: string }
  | { kind: "fail"; reason: string };

export interface ImportedHost {
  name: string;
  address: string;
  folder: string;
  fields: { field: HostField; value: string }[];
  /** The port setting, if the import gives one. */
  port: number | null;
  tags: string[];
  custom: CustomField[];
}

export interface PreviewRow {
  line: number;
  name: string;
  outcome: PreviewOutcome;
  host: ImportedHost | null;
}

export interface ImportPreview {
  rows: PreviewRow[];
  /** Folders the import would create, parents first. */
  newFolders: string[];
  /** Columns that are never written (passwords). */
  skippedColumns: ReportEntry[];
}

export interface ImportReport {
  created: ReportEntry[];
  /** Existing hosts the user confirmed to take the imported values. */
  updated: ReportEntry[];
  skipped: ReportEntry[];
  failed: ReportEntry[];
  createdFolders: string[];
  /** Written, but the workspace could not be reloaded afterwards: why. */
  reloadError: string | null;
}

/** What importing a Nornir inventory would do. */
export interface NornirPreview {
  /** One per host of the hosts file, in file order. */
  hosts: NornirPreviewHost[];
  /** Never imported: passwords, usernames, connection options, ... */
  skipped: ReportEntry[];
}

export interface NornirPreviewHost {
  /** The line of the host's key in the hosts file, if found. */
  line: number | null;
  name: string;
  outcome: PreviewOutcome;
  host: ImportedHost | null;
}

/** Asks for a Nornir inventory file; null if none was picked. */
export async function pickNornirFile(title: string): Promise<string | null> {
  const file = await open({
    title,
    filters: [
      { name: "YAML", extensions: ["yaml", "yml"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
  return typeof file === "string" ? file : null;
}

/**
 * Reads a Nornir simple inventory and shows what importing it into `target`
 * would do; writes nothing. The import then uses what this read.
 */
export function previewNornir(
  hosts: string,
  groups: string | null,
  defaults: string | null,
  target: string,
) {
  return invoke<NornirPreview>("nornir_import_preview", {
    hosts,
    groups,
    defaults,
    target,
  });
}

/**
 * Runs the Nornir import the last preview showed (its files as read then,
 * into its target). Existing hosts are only written if their id is in
 * `replace`.
 */
export function importNornir(replace: string[]) {
  return invoke<Edited<ImportReport>>("nornir_import_run", { replace });
}

/** Asks for a CSV file to import; null if none was picked. */
export async function pickCsvFile(): Promise<string | null> {
  const file = await open({
    title: "CSV file to import",
    filters: [
      { name: "CSV", extensions: ["csv", "txt"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
  return typeof file === "string" ? file : null;
}

/** Reads a CSV file for importing; the preview and import then use it. */
export function readCsv(path: string) {
  return invoke<CsvFileView>("csv_import_read", { path });
}

/** What importing the CSV file read last would do; writes nothing. */
export function previewCsv(mapping: ColumnTarget[], target: string) {
  return invoke<ImportPreview>("csv_import_preview", { mapping, target });
}

/** One of the fields filling in from devices sets. */
export type EnrichField = "vendor" | "model" | "serial_number" | "os_version";

/** One field a host would get from its device. */
export interface FieldChange {
  field: EnrichField;
  /** `new`: the field is empty; `replace`: it has another typed value. */
  kind: "new" | "replace";
  current: string | null;
  value: string;
}

/** What filling in would do to one host. */
export interface EnrichPreviewHost {
  id: string;
  name: string;
  outcome:
    | { kind: "changes" }
    | { kind: "unchanged" }
    | { kind: "skipped"; reason: string }
    | { kind: "failed"; reason: string };
  changes: FieldChange[];
}

export interface EnrichPreview {
  hosts: EnrichPreviewHost[];
}

export interface EnrichReportedHost {
  id: string;
  name: string;
  /** The fields written. */
  fields: EnrichField[];
  /** The fields whose typed value was kept (the host was not ticked). */
  kept: EnrichField[];
  reason: string | null;
}

export interface EnrichReport {
  filled: EnrichReportedHost[];
  unchanged: EnrichReportedHost[];
  skipped: EnrichReportedHost[];
  failed: EnrichReportedHost[];
  reloadError: string | null;
}

/** What filling in the facts of the kept run `run` would change. */
export function previewEnrich(run: string) {
  return invoke<EnrichPreview>("enrich_preview", { run });
}

/**
 * Fills in the preview made last of `run`: empty fields, and typed values
 * only on the hosts in `replace`.
 */
export function applyEnrich(run: string, replace: string[]) {
  return invoke<Edited<EnrichReport>>("enrich_apply", { run, replace });
}

/**
 * Imports the CSV file read last into `target`. Existing hosts are only
 * written if their id is in `replace`.
 */
export function importCsv(
  mapping: ColumnTarget[],
  target: string,
  replace: string[],
) {
  return invoke<Edited<ImportReport>>("csv_import_run", {
    mapping,
    target,
    replace,
  });
}

/** Returns the host's new id. */
export function renameHost(id: string, name: string) {
  return invoke<Relocated<string>>("host_rename", { id, name });
}

/** Returns the host's new id. */
export function moveHost(id: string, folder: string) {
  return invoke<Relocated<string>>("host_move", { id, folder });
}

export function deleteHost(id: string) {
  return invoke<Edited<null>>("host_delete", { id });
}

/** Saves a host's fields (not its name or place: see rename and move). */
export function saveHost(host: Host) {
  return invoke<Edited<null>>("host_save", { host });
}

/** The hosts matching every word of `query`, best first. */
export function searchHosts(query: string, limit: number) {
  return invoke<SearchResults>("workspace_search", { query, limit });
}

export function applyFilter(filter: SavedFilter) {
  return invoke<FilterView>("filter_apply", { filter });
}

/** Saves a filter, replacing the one of the same name. */
export function saveFilter(filter: SavedFilter) {
  return invoke<Edited<null>>("filter_save", { filter });
}

export function deleteFilter(name: string) {
  return invoke<Edited<null>>("filter_delete", { name });
}

/** What to highlight in each line, by the open workspace's rules. */
export function highlightLines(lines: string[]) {
  return invoke<HighlightSpan[][]>("highlight_lines", { lines });
}

/** The built-in highlight rules (for when no workspace is open). */
export function builtInHighlightRules() {
  return invoke<ListedRule[]>("highlight_built_in_rules");
}

/** Saves a rule in place of the one called `previous`, or as a new one. */
export function saveHighlightRule(
  previous: string | null,
  rule: HighlightRule,
) {
  return invoke<Edited<null>>("highlight_rule_save", { previous, rule });
}

/** Deletes a workspace rule, or a built-in rule's changes. */
export function deleteHighlightRule(name: string) {
  return invoke<Edited<null>>("highlight_rule_delete", { name });
}

/** Saves a platform definition in place of `previous`, or as a new one. */
export function savePlatform(
  previous: string | null,
  definition: PlatformDefinition,
) {
  return invoke<Edited<null>>("platform_save", { previous, definition });
}

/** Deletes a workspace platform definition, or resets a built-in one. */
export function deletePlatform(name: string) {
  return invoke<Edited<null>>("platform_delete", { name });
}

/** The snippets with every word of `query` in their name, group or text. */
export function findSnippets(query: string) {
  return invoke<Snippet[]>("snippet_find", { query });
}

/** Saves a snippet in place of the one called `previous`, or as a new one. */
export function saveSnippet(previous: string | null, snippet: Snippet) {
  return invoke<Edited<null>>("snippet_save", { previous, snippet });
}

export function deleteSnippet(name: string) {
  return invoke<Edited<null>>("snippet_delete", { name });
}

/** Saves a job in place of the one called `previous`, or as a new one. */
export function saveJob(previous: string | null, job: SavedJob) {
  return invoke<Edited<null>>("saved_job_save", { previous, job });
}

export function deleteJob(name: string) {
  return invoke<Edited<null>>("saved_job_delete", { name });
}

/**
 * The variables of `step`'s answers a saved job does not keep, as their
 * names look like secrets (#126): they are asked at each run.
 */
export function secretAnswers(step: JobStep) {
  return invoke<string[]>("saved_job_secret_answers", { step });
}

/**
 * The lines of the snippet `name` for a session to the inventory host
 * `host` (null: no inventory host), its variables filled from the host's
 * fields and then from `answers`; or the variables still to ask for.
 */
export function expandSnippet(
  name: string,
  host: string | null,
  answers: Record<string, string>,
) {
  return invoke<Expansion>("snippet_expand", { name, host, answers });
}

/**
 * The snippet `name` for the session `id`, filled from its host while the
 * open workspace is its own (the shell knows it, #86), then `answers`.
 */
export function expandSnippetForSession(
  id: number,
  name: string,
  answers: Record<string, string>,
) {
  return invoke<Expansion>("session_snippet_expand", { id, name, answers });
}

/** A snippet made ready for each member of the multi-send group. */
export type GroupExpansion =
  { kind: "lines"; members: string[][] } | { kind: "missing"; names: string[] };

/**
 * The snippet `name` for each member of the multi-send group: `hosts` holds
 * each member's live host (null: none, it gets only the answers).
 */
export function expandSnippetForGroup(
  name: string,
  hosts: (string | null)[],
  answers: Record<string, string>,
) {
  return invoke<GroupExpansion>("snippet_expand_group", {
    name,
    hosts,
    answers,
  });
}

export function findFolder(tree: Folder, path: string): Folder | null {
  if (path === "") return tree;
  let folder: Folder | undefined = tree;
  for (const name of path.split("/")) {
    folder = folder.folders.find((child) => child.name === name);
    if (!folder) return null;
  }
  return folder;
}

export function findHost(tree: Folder, id: string): Host | null {
  const slash = id.lastIndexOf("/");
  const folder = findFolder(tree, slash < 0 ? "" : id.slice(0, slash));
  return folder?.hosts.find((host) => host.id === id) ?? null;
}

/** The folder path a host or folder is in. */
export function parentPath(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

/** Every host of the tree, depth first. */
export function allHosts(tree: Folder): Host[] {
  return [...tree.hosts, ...tree.folders.flatMap(allHosts)];
}

/** Every folder of the tree, root first, depth first. */
export function allFolders(tree: Folder): Folder[] {
  return [tree, ...tree.folders.flatMap(allFolders)];
}
