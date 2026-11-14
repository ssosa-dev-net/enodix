// The interface's view of the core's workspace: the tree it loaded, the
// files it could not load, and the edits it can make. Everything here is a
// call into the Tauri shell; the core writes the files and returns the
// reloaded workspace.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export interface CustomField {
  key: string;
  value: string;
}

export type Protocol = "ssh" | "telnet" | "serial";
export type Environment = "production" | "staging" | "lab";
export type Parity = "none" | "odd" | "even" | "mark" | "space";
export type FlowControl = "none" | "hardware" | "software";

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
  environment: Environment | null;
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
  environment: Effective<Environment> | null;
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
  platform: string | null;
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
  /** The settings the host sets itself. */
  settings: Settings;
}

/** The optional built-in text fields of a host, in display order. */
export const HOST_TEXT_FIELDS = [
  ["vendor", "Vendor"],
  ["platform", "Platform"],
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

export interface WorkspaceView {
  /** Higher is newer: a view older than the one shown is stale. */
  revision: number;
  root: string;
  tree: Folder;
  /** The roles a host can be given. */
  roles: Role[];
  /** The role each host that has one shows (neutral if undefined), by id. */
  host_roles: Record<string, Role>;
  /** The saved filters, in file order. */
  filters: SavedFilter[];
  /** The highlight rules: the workspace's own first, then the built-in. */
  highlight_rules: ListedRule[];
  problems: Problem[];
}

/** The workspace after an edit, and what the edit returned. */
export interface Edited<T> {
  workspace: WorkspaceView;
  result: T;
}

export type FolderDeleted =
  | { kind: "deleted"; workspace: WorkspaceView }
  | { kind: "needsConfirmation"; hosts: number };

/** Asks the user for a workspace folder; null if they cancel. */
export async function pickWorkspaceFolder(): Promise<string | null> {
  const folder = await open({
    directory: true,
    title: "Open workspace folder",
  });
  return typeof folder === "string" ? folder : null;
}

/** Opens a workspace folder; it reopens on the next launch. */
export function openWorkspace(path: string): Promise<WorkspaceView> {
  return invoke<WorkspaceView>("workspace_open", { path });
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

/** Reopens the last opened workspace, if any. */
export function restoreWorkspace(): Promise<WorkspaceView | null> {
  return invoke<WorkspaceView | null>("workspace_restore");
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
  return invoke<Edited<string>>("folder_rename", { path, name });
}

/** Returns the folder's new path. */
export function moveFolder(path: string, parent: string) {
  return invoke<Edited<string>>("folder_move", { path, parent });
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

/** Returns the host's new id. */
export function renameHost(id: string, name: string) {
  return invoke<Edited<string>>("host_rename", { id, name });
}

/** Returns the host's new id. */
export function moveHost(id: string, folder: string) {
  return invoke<Edited<string>>("host_move", { id, folder });
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
