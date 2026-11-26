# Workspace

The workspace folder format and everything read from it (`crates/core/src/workspace.rs`, `src/workspace/`), with the shell commands and interface parts that show it.

- `src/workspace.rs` — loads a workspace folder into a tree (`Workspace::open`)
  and edits it: `create/rename/move/delete_folder`, `set_folder_description`,
  `create/rename/move/delete_host`, `write_host` (saves fields). Every edit
  writes the files, then reloads the tree in memory; a refused edit
  (`EditError`) touches no file. Format: directories are folders; each
  `*.yaml`/`*.yml` file is a host (`name` (defaults to the file stem),
  `address` = management address (required), `vendor`, `platform`,
  `model`, `serial_number`, `site`, `building`, `room`, `rack`,
  `rack_unit`, `role` (a role id), `description`, `tags` (list), `custom` (mapping of the
  user's own fields), plus the inheritable settings below);
  `_folder.yaml` is a folder's settings file (`description`, `tags`, the
  inheritable settings); other `_*` files are reserved for workspace-wide
  definitions; dot-entries are ignored; symlinked folders are not followed.
  A host keeps its whole YAML mapping, so unknown keys survive a rewrite in
  their original order; YAML comments survive only if the host is saved
  unchanged (then the file is not written at all). Unchanged values keep
  their exact YAML form; a new built-in key goes after its nearest
  predecessor in `HOST_KEY_ORDER`. A bad
  file becomes a `Problem` (file path + message) and never stops loading.
  `HostId` is the host file's `/`-separated path relative to the root;
  folder paths are the same without a file name (`""` is the root). Names
  must be portable file names; clashes are checked ignoring case, the
  Unicode form (#132, below) and the host file extension; an empty one is "the name is empty" (#116). A new
  address with one colon and digits after it (`10.0.0.1:2222`,
  `router1:22`) is refused by `create_host` and by `write_host` when the
  address changes: "write the address without the port (…), then set
  Port to … in the host's details"; since #131 so is one with a space
  inside it ("the address has a space in it (…): write one host name or
  IP address"; around it is trimmed) (`one_address` over the
  `pub(crate) bad_address` → `BadAddress::Space | Port { host, port }`,
  which the imports share; IPv6 has more colons
  and is kept; an existing file with such an address is not refused on
  other edits). `delete_folder` refuses a folder holding hosts
  unless given `FolderDeletion::IncludingHosts` (the interface's
  confirmation). Known gap: serde_norway writes text like `yes` unquoted,
  which YAML 1.1 readers (PyYAML/Nornir) take as a boolean.
- `src/workspace/settings.rs` — inherited settings. Keys, the same in
  host files and `_folder.yaml`: `protocol` (ssh/telnet/serial), `port`,
  `platform` (any text; #40), `credential_profile` (a name), `jump_host` (a host id; see `jumps.rs`), `legacy_crypto`, `logging`, `backup_on_login` (#46, off by default) (bools),
  `connect_timeout` (seconds, 1–3600, default 15 = `session::DEFAULT_CONNECT_TIMEOUT`),
  `terminal_type` (#24: `xterm-256color` default, `xterm`, `vt100`, `ansi`, `linux`;
  `TerminalType`, carried by `SshTarget` and `TelnetTarget` and used for the pty
  request and telnet's terminal type; a running session keeps its own), `environment`
  (production/staging/lab), `web_links` (mapping name → URL template),
  `serial` (mapping: `port` (e.g. `COM3`), `baud_rate`, `data_bits` 5–8,
  `parity`, `stop_bits` 1–2, `flow_control`; each inherits on its own;
  `EffectiveSerial.port` is `None` when unset). `Host.settings` /
  `Folder.settings` (+ `Folder.tags`) are what a level sets itself
  (`Host.port` is gone: use `settings.port`; likewise `Host.platform`:
  `settings.platform`, read as before (number → text, blank unset), its
  host key still goes after `vendor`, folder key after `tags`); `Workspace::resolve_host`
  → `ResolvedHost { host, settings: EffectiveSettings }` and
  `resolve_folder` give what applies, each value an `Effective { value,
origin: Default | Host | Folder{path} }`. Defaults: ssh, logging on,
  legacy off, 9600 8N1 no flow control; `port`, `platform`, `credential_profile`,
  `jump_host`, `environment` are `None` when unset (a `None` port means
  the protocol's default: `ResolvedHost::ssh_target` uses 22,
  `telnet_target` 23). Links merge by name (first position kept, nearer one
  replaces); tags add up, once each, with the nearest origin. Edit with
  `write_host` (host) or `write_folder_settings` / `set_folder_tags`;
  only changed keys are touched, new keys go by `HOST_KEY_ORDER` /
  `FOLDER_KEY_ORDER`, unknown keys inside `serial` survive. Always
  connect through `Profiles::profile_target` / `prompted_target` (which
  resolve the host and add its jump hosts), never a bare `Host`;
  `ResolvedHost::ssh_target` leaves the jump hosts out.
- `src/workspace/roles.rs` — device roles. `Workspace::roles()` = the 10
  built-in roles, then the workspace's own from `_roles.yaml` at the
  root only (mapping id → `name` (optional), `icon` (a `RoleIcon` name),
  `color` (`#rgb`/`#rrggbb`, stored lower-case `#rrggbb`)); a workspace
  role with a built-in id replaces it in place. Bad definitions are
  `Problem`s on `_roles.yaml` and are left out. Hosts name a role by id,
  matched ignoring case; `Workspace::host_role` gives the role, or a
  neutral `Role::undefined` (icon `unknown`, `NEUTRAL_ROLE_COLOR`) when
  the workspace does not define it. The shell's `WorkspaceView` carries
  `roles` and `host_roles` (`Workspace::host_roles`: host id → role); the interface draws icons
  in `RoleIcon.svelte` (add a shape there when adding a `RoleIcon`).
  No UI edits `_roles.yaml` yet.
- `src/workspace/search.rs` — the search index, built in `Workspace::open`
  (so every edit's reload rebuilds it). `Workspace::search(query, limit)`:
  every whitespace word must be part of some field, ignoring case (name,
  address, tags incl. inherited, built-in fields, role id and name, custom
  values); ranked by summed per-word score (field weight × 4 + match kind:
  whole/start/word start/anywhere), then name. Empty query = every host by
  name. ~7 ms per query on 5,000 hosts in debug (`tests/search.rs` times it).
- `src/workspace/filters.rs` — saved filters in `_filters.yaml` at the
  root (name → conditions: built-in keys, `tags` (scalar or list),
  `custom` mapping, `text`). In the API a `Condition { field, value }` has
  `field` = a built-in key, `tag`, `custom.<key>` or `text`; values match
  whole ignoring case, `*` wildcard. `Workspace::filters`, `apply_filter`
  (→ `FilterView { hosts, folders }`, folders = ancestors, root excluded),
  `save_filter` (replaces same name ignoring case, in place),
  `delete_filter`. Bad definitions are `Problem`s on `_filters.yaml`.
  Writing a workspace-wide file goes through `Workspace::edit_file`.
- `src/workspace/watch.rs` — live reload. `WorkspaceWatcher::start(root,
WatchOptions, on_change)` (the `notify` crate, recursive) calls
  `on_change(Changes { files })` on its own thread once per burst: after
  `quiet` (300 ms) without events, at most `max_delay` (1.5 s) after the
  first. Hidden paths (`.git`, the app's `.x.tmp` files) and reads are
  dropped; so are Windows' late "folder modified" events (a reload's own
  `read_dir` triggers them, which looped). Dropping the watcher stops it.
  The owner then calls `Workspace::refresh()` → `true` only if the
  tree/roles/filters/problems differ, so the app's own edits (already
  reloaded) cause no update. The shell (`Shell::open`) starts a watcher
  per opened workspace (then refreshes once, for changes made before it
  started) and emits `workspace-changed` with a `WorkspaceView`. Each
  view carries `Workspace::revision()` (+1 per reload that wrote or found
  a change), and the interface (`adopt` in `inventory.svelte.ts`) ignores
  views older than the one shown. `workspaceChanged` keeps selection and
  expansion, deselecting only an item that is gone. Sessions never hold
  workspace data. Known limits: a pull longer than `max_delay` gives
  more than one refresh. The folder itself (#132): the thread owns the
  system watch (`notify`) and every `check_every` (2 s) checks the root's
  identity (`folder_identity`: device and inode on Unix, volume serial and
  file index through `GetFileInformationByHandle` on Windows, the folder
  opened only for that): gone → the watch is dropped and one empty
  `Changes` is sent (the owner's refresh then fails: unreachable); back,
  or another folder at the path, → watched anew and one empty `Changes`.
  `WorkspaceWatcher::check_again()` (the shell calls it after a refresh
  found the folder unreachable) makes the next check watch it anew even
  with the same identity (a share whose connection drops ends notify's
  watch without a word: notify unwatches on an unknown error). Drop
  drops the system watch at once and stops the thread with a message.
  Unreachable and another workspace (`src/workspace/reach.rs`):
  `Workspace::unavailable() -> Option<&Unavailable>`, `Unavailable::
  Unreachable { reason } | OtherWorkspace { id }` (Display "the workspace
  folder cannot be reached (it is not there: a network drive that
  disconnected, or a folder moved or removed)" / "the workspace folder is
  now another workspace (its _workspace.yaml names …)", `explanation()`).
  `refresh` sets it when the open fails, or when the open workspace had
  an id and the folder's `_workspace.yaml` now names another or none
  (credential profiles, kept runs and config history hang on the id; the
  user reopens the folder from the workspaces to take it as it is now);
  a change of it is a new revision; the refresh that loads the folder
  again clears it (and counts as a change). Meanwhile what was loaded is
  shown and every edit is refused, before anything is written, with
  `EditError::Unavailable` ("…, so nothing was saved"; checked in
  `existing_folder`, `existing_host`, `edit_file`, which also check the
  root is a folder, so an edit before any refresh is refused alike).
  `refresh_with(Result<Workspace, OpenError>)` = `refresh` with a
  workspace loaded elsewhere: the shell's `reload_from_disk` loads the
  folder without holding its workspace lock and takes it holding it
  (dropped when the revision moved meanwhile: an edit's own reload is
  newer), emits a view whenever the revision changed (so also for
  unreachable), and calls `check_again` while unreachable. Measured
  (release, Windows 11, 20 000 hosts flat, one file changed, best of 3,
  `edge_inventory_scale` `a_refresh_of_a_large_inventory_holds_the_
  workspace_only_to_take_it`): lock held 2.1 s for the whole refresh,
  27 ms to take one loaded beside, then 32 ms building the view (the
  shell's `WorkspaceView::from` under the lock, unchanged). Interface:
  `WorkspaceView.unavailable` (`{ kind, reason | id, message,
  explanation }`), HostsView's notice `[data-workspace-unavailable=
  unreachable|otherWorkspace]` (and `[data-workspace-unavailable-open]`,
  the workspaces dialog, for another workspace). Since #117 an app save of a host whose file
  changed, broke or went since it was loaded is refused
  (`EditError::ChangedOutside`, nothing written; `write_host`, and
  `rename_host` when it rewrites `name:`), so an external edit is never
  written over, however late the watcher is; the edit works again after
  the reload. Tests: `tests/workspace_watch.rs`, `tests/edge_inventory*.rs`.
- Edge testing (#117, `tests/edge_inventory*.rs`). Lookups: `Workspace`
  keeps `Places` (built in `open`): each folder path → the indices down to
  it, each host id → its index in its folder, each lower-case id → the
  first host in tree order; `host`, `folder`, `levels` and
  `find_host_id`'s ignoring-case fallback use it. Before, `host(id)`
  walked the folder's hosts, so planning a job on a folder of 20 000 hosts
  took 4.4 s in release (quadratic) and exporting 0.77 s; now 0.44 s and
  0.16 s. `inventory::export_within` uses sets for the job's scope and
  for `Limit::Hosts` (a script job's 20 000 ids was quadratic too). Times
  (release, Windows 11, a busy machine, best of 3; open/refresh best of 2):
  5 000 flat open 0.77 s, tree 7 ms, search 2 ms, filter 2 ms, refresh
  0.9 s, export 48 ms, plan 0.19 s; 20 000 flat 4.5 s, 44 ms, 6 ms, 9 ms,
  3.3 s, 0.16 s, 0.44 s; 20 000 deep (500 rooms of 40) 3.9 s, 31 ms, 5 ms,
  5 ms, 2.8 s, 0.17 s, 0.46 s. Loading is
  file reading (about 0.15 to 0.2 ms a host on Windows); a refresh is a
  whole load, done under the shell's workspace lock until #132 (20 000
  hosts held the interface's workspace commands for seconds on each
  external change); now loaded beside the lock (`refresh_with`, above).
  Files: `read_mapping` drops a leading byte order mark (before, a
  `_folder.yaml` or host starting with a key failed with "more than one
  document"); every kind cut anywhere, of wrong types, ANSI or UTF-16
  (named "cannot read file: stream did not contain valid UTF-8"), tabs as
  indentation: a problem on that file only, the rest loads; the edits
  (`edit_file` re-reads and refuses a file it cannot read) never write
  over one; unknown keys and bad entries survive edits of the other
  entries. Hostile YAML: an alias bomb is refused at once; nesting took
  about a second to refuse at 10 000 levels, 35 s at 100 000 (libyaml's
  scanner is quadratic in depth); since #132 `read_mapping` (every
  workspace file) first runs `depth::nested_deeper_than(text,
  MAX_NESTING = 32)`, one pass that skips quoted, comment and block
  scalar text, counts `[`/`{` only where a value starts or inside flow,
  and block levels by indentation plus `- `/`? ` on one line (it can
  count too few, never too many, for a file YAML reads): "it is nested
  deeper than 32 levels, more than any workspace file needs, so it is not
  read", a problem like any unreadable file, never written over
  (`edit_file` re-reads). 100 000 levels of `[`, `{a: `, `- ` and 200
  levels of indentation now take milliseconds. The Nornir import's
  `hosts.yaml` is parsed elsewhere and not checked. Names: `is_device_name`
  also refuses `COM0`, `LPT0`, `COM¹²³`, `LPT¹²³`, `CONIN$`, `CONOUT$`;
  300-character names fail with the system's error, nothing written;
  paths over 400 characters work on Windows. Two Unicode forms of a name
  (#132, `src/names.rs`: `composed` = NFC, `folded` = lower case then
  NFC, `same_name`, `same_trimmed_name`; the `unicode-normalization`
  crate) are one name wherever a name is checked free or something is
  found by name: `ensure_free` (create, rename, move of hosts and
  folders), the imports' clash maps (`existing_hosts`, `Entries`, CSV's
  planned rows and new folders, Nornir's planned names),
  `Places.lower`/`find_host_id` (jump host references, `Limit::Hosts`),
  `host_named`, `moves::moved_host` (references rewritten on a move),
  export's folder and filter by name, search (index values, query words,
  filter conditions), `find_snippets`, the names of filters, snippets,
  jobs, highlight rules and platforms, and credential profiles
  (`Profiles::profile` tries the name as written, then composed; `save`
  stores the name composed; `missing` compares folded). A file keeps the
  name it has; two entries of a folder whose names are equal composed but
  not as written (hosts by file stem, folders by name; not case-only
  differences) both load, and the second is a problem "this host has the
  name of … written in two Unicode forms …" (`two_forms_of_one_name`).
  Left: the keychain and vault keys (`name.to_lowercase()`; a profile
  stored under a decomposed name, which the app's form does not make, is
  found only as written), named secrets, template variable names, role
  ids (ASCII in practice), `platform_names` keys sent to the interface;
  case is folded with Unicode lower case, not NTFS's upcase table (`ß`,
  dotted and dotless i may differ from what Windows calls one name). On
  Linux, two files differing in
  case are two hosts, exact references win. Changes under the app: a pull
  rewriting 400 files (50 removed, 50 added, a folder renamed), a branch
  switch replacing the tree, an editor saving by rename (or after a `~`
  backup; its `.swp` and `.tmp` files are never hosts) end with the tree on
  disk; a folder renamed outside: edits of the old path are refused and
  never make it again. The workspace folder going away: `refresh` fails
  and keeps what was shown, every edit is refused and none makes the
  folder again, and once back it all works; the watcher goes on after the
  folder is moved away and back, and since #132 after it is removed and
  made again (a network drive reconnecting; the watch above), the window
  saying so meanwhile. Smoke: `liveReload` removes its workspace folder
  (`[data-workspace-unavailable=unreachable]` "cannot be reached"),
  makes it again from a copy, and checks a host added after shows.
- Web links (#24, `src/workspace/web_links.rs`). Merging is the
  `web_links` setting (above). `web_url(text)` (the `url` crate) →
  normalised URL, or `WebLinkError::NotAUrl | NotWeb { scheme } |
HoldsPassword` (only http/https; `user:password@` refused, the password
  not echoed); `WebLink::open_url(&Variables)` = `fill` then `web_url`
  (`Missing`); `ResolvedHost::web_links()` → `HostWebLink { name,
template, origin, url: Result }` (JSON: `url` or `refused` text),
  `web_link_url(name)` (exact name, as merging; `NoSuchLink`). Values are
  not percent-encoded (they may fill a host or path). Shell:
  `host_web_links(id)`, `web_link_open(id, name)` (tauri-plugin-opener
  `open_url`; with `ENODIX_WEB_LINKS_TO=<file>` it appends the URL there
  instead: the smoke's seam, never a browser). Interface: HostDetails'
  `[data-ways-in]` row = connect buttons + `[data-web-link=name]`
  (`[data-refused]` when the core refuses it; click still asks the core),
  message `[data-web-link-error]`. Smoke: `webLinks`.
- Multiple workspaces (#25, `src/workspace/identity.rs`, `src/app_state.rs`).
  Identity: `_workspace.yaml` at the root (`id`: letters, digits, `-_.`,
  ≤64; a uuid v4 when the app writes it, with a comment header).
  `Workspace::id()`, `ensure_id()` (writes the file if missing, adds `id`
  to a file without one keeping other keys, never overwrites a bad file:
  that is a `Problem` and an error), `Workspace::create(root)` (creates
  the folder; refuses one holding anything but dot-entries, "the folder
  is not empty; open it as a workspace instead, or create one in an
  empty folder"; writes the id). `Workspace::open` says a missing folder
  and a file in the user's words, with a way on, not the system's "os
  error 2" (#116, `tests/workspaces.rs`). Opening a folder that is not a
  workspace asks first (#131, below). `credentials::prepare_workspace(store, &mut ws)` = `ensure_id` +
  move profiles from the old path scope to the id (an id-scoped profile of
  the same name wins; the old one is left). The shell runs it in
  `Shell::open` for every workspace it opens (so opening an existing
  folder writes `_workspace.yaml` into it; a failure is added to the
  view's `problems` on `_workspace.yaml`, profiles then stay under the
  path). Known limits: a copied workspace folder shares its id (and
  profiles) until its `_workspace.yaml` is deleted; two clones opened
  before the file is committed get different ids (a git conflict); if the
  file is deleted (or a checkout lacks it) the next open writes a new id
  and the profiles under the old one are no longer found.
  `AppState { last_workspace, workspaces }` in `<data folder>/state.yaml`
  (the shell picks the folder: Tauri's app data dir, or
  `ENODIX_DATA_DIR`, which the smoke test sets): `remember(path)`
  (to the top, stored as `app_state::resolved_path`: canonical without
  `\\?\`, else absolute; `same_folder` compares those), `forget(path,
open)` (list only, never the folder; `ForgetError::Open` for the open
  one; clears `last_workspace` if it was it),
  `known_workspaces(open)` → `KnownWorkspace { path, name, open,
available }`; `load` adds an old file's `last_workspace` to the list and moves a
  damaged file to `state.yaml.damaged` (so the next save loses nothing).
  Shell: `workspace_open` (adds an existing folder), `workspace_create`,
  `workspaces_list`, `workspace_forget`; both open commands open the
  `resolved_path`, so `WorkspaceView.root` is one string per folder (the
  interface compares roots as strings); state edits go through
  `Shell::update_state` (a mutex). `known_workspaces` checks every
  folder exists (`is_dir`): a disconnected network drive may slow it. Interface:
  `WorkspacesDialog.svelte` (`[data-dialog=workspaces]`, opened by the
  Hosts view's workspace name `[data-open-workspaces]` or Ctrl+Shift+O; filter input
  `workspace-query`, rows `[data-workspace=<path>]`
  (`[data-workspace-open]`), `[data-workspace-remove]`, forms
  `[data-workspace-form=create|add]` with a `workspace-folder` input +
  Browse…, `[data-workspace-error]`). `switchWorkspace` in `App.svelte`:
  if sessions to hosts of the open workspace are live, `ConfirmSwitch.svelte`
  (`[data-confirm-switch]`, `[data-switch=cancel|close|keep]`) asks; close
  = their tabs close after the other workspace opened. Each `OpenSession`
  keeps `workspace` (root) and a `role` snapshot; `liveHost(session)` is
  its host id only while its workspace is open, and role/safety/snippet
  values/password fallback use it (a kept session of another workspace
  shows what it opened with and gets no host values). Smoke: `workspaces`.
  Not in scope: two workspaces whose folders share a name still share a
  log folder (see #20).
- What kind of folder (#131, `src/workspace/kind.rs`). Opening any folder
  used to make it a workspace without a word (`prepare_workspace` writes
  `_workspace.yaml`; its subfolders show as host folders). `workspace::
  folder_kind(root) -> Result<FolderKind, OpenError>` decides, reading
  only: `Workspace` = a host file (a `*.yaml`/`*.yml` not starting with
  `_`, at most 256 KiB, whose top-level mapping has `address`; in the
  folder or any subfolder) or one of a workspace's own files (at the
  root: `_workspace.yaml`, `_folder.yaml`, `_roles`, `_filters`,
  `_platforms`, `_snippets`, `_highlights`, `_jobs` `.yaml`; below it:
  `_folder.yaml`); `Empty` = nothing but dot-entries and folders holding
  nothing else (a workspace laid out by hand before its first host);
  `Other` = anything else (files of other kinds, YAML that is not a host:
  `docker-compose.yml`, `_config.yml`, a list, broken or blank YAML).
  Walked breadth first as `open` loads (dot-entries skipped, links to
  folders not followed, counted as other), stopping at the first sign of a
  workspace; after 20 000 entries with none it is `Other` (a whole
  Documents folder is not walked). Refuses a missing root, a file or an
  unlistable folder with `open`'s words (`check_root`, shared). Shell:
  `workspace_folder_kind(path)`; `workspace_open(path, confirmed?)`
  refuses an `Other` folder unless `confirmed` ("… holds no host files and
  is not a workspace; nothing was written into it"), so nothing is written
  without the user's yes however the interface calls it;
  `workspace_restore` refuses (same words + "; open it from the
  workspaces to make it one again") a last workspace that is `Other` now
  (its files gone), shown as the Hosts view's open error, the welcome view
  then shown. Interface: App's `openFolder(path)` (Welcome and
  WorkspacesDialog take it as `onopen`; every open of an existing folder
  goes through it: a known row, a typed or browsed path) asks `folderKind`,
  and for `other` shows `ConfirmOpenFolder.svelte` (window-modal,
  `[data-confirm-open-folder=<path>]`, `[data-confirm-folder-path]`,
  buttons `[data-confirm-folder=cancel|open]`, Cancel focused, Escape
  cancels): "Make this folder a workspace?", the path, "No host files
  here, and it is not a workspace yet. Opening it makes it one: a small
  file, _workspace.yaml, is written into it, and its subfolders show as
  host folders." Cancel resolves to no error and writes nothing; yes calls
  `openWorkspace(path, true)` through `switchWorkspace` (the
  live-sessions question comes after). Create is unchanged (it refuses a
  non-empty folder). Tests: `tests/folder_kinds.rs` (each kind, hidden
  entries only, empty subfolders only, `_workspace.yaml` alone, each
  workspace file, non-host YAML, missing/file root, and that `open` alone
  writes nothing; every case checks the folder is unchanged). Smoke:
  `welcome` (a typed folder of other things: the question's words and
  focus, nothing written before or after Cancel) and `workspaces` (the
  switcher's add form: Escape cancels the question only, nothing written;
  then yes: it opens, `_workspace.yaml` the one file written; back and
  off the list). The headless binary only opens (never `ensure_id`): it
  never wrote, and now warns (`cli.md`). Not verified: a folder on a slow
  network drive (the walk may take a while before the question), the
  restore refusal in the app (no smoke step: the smoke's last workspace
  is a workspace).
- Platform as an inherited setting (#40, `settings.rs`). `Settings.platform`
  (host files and `_folder.yaml`, key `platform`), `EffectiveSettings.
platform: Option<Effective<String>>` (`None` when nothing sets it).
  Readers use the effective value: the search index (`SearchIndex::add`
  passes the nearest folder's platform down; a host's own wins), so
  saved filters' `platform` condition too, and `ResolvedHost::variables`
  (`{{platform}}`; a custom field named `platform` is only
  `custom.platform`). Writers (`write_host`, CSV/Nornir imports via
  `text_field_mut`) write the host's own. Interface: `Host.platform` is
  gone (`settings.platform`), the HostDetails text field moved to
  SettingsPanel's "Device" section (`setting-platform`,
  `[data-origin=platform]`, datalist `platforms` = every platform the
  workspace names). Tests: `settings.rs` (inherit fixture: Europe sets
  `eos`, `Europe/Madrid/core-r1` its own `srlinux`), `search.rs`,
  `filters.rs`, `snippets.rs`. Smoke: `editInventory` sets/clears it on
  a host and sets it on a folder. Resolving it to a definition: #41.
- Platform definitions (#41, `src/platform.rs`, `src/workspace/platforms.rs`).
  `PlatformDefinition { name, other_names, prompt, disable_paging,
privilege_command, privilege_prompt, show_config, ignored_lines,
file_folder }`
  (camelCase over IPC; patterns = `prompt`, `privilege_prompt`,
  `ignored_lines`, `regex` syntax; `check()` names each bad one).
  `file_folder` (#102): where files belong on the device, as it writes
  the path; not a pattern; trimmed when read and saved; built in eos `/mnt/flash`,
  junos `/var/tmp`, ios `flash:`, nxos `bootflash:`, srlinux and frr none
  (eos, junos, ios checked on the lab in #96, `docs/containerlab.md`
  "Files on the lab devices"; nxos not, no image; `flash:` on physical
  Catalyst hardware not). `Workspace::host_file_folder(id) ->
Option<&str>` answers through `host_platform` (effective platform, a
  folder's too): `None` without a platform, a usable definition or the
  value. For the Files tab's start (#103), the SCP form's default path
  (#106) and the file job's destination (#107); no shell command yet.
  `BUILT_IN_PLATFORMS`: srlinux, eos, frr (vtysh), ios, nxos, junos, with
  Nornir/Netmiko/NAPALM names as `other_names` (prompts of srlinux/eos
  from the lab suite; every other value unverified until #47). `_platforms.yaml`
  at the root: name → those keys (`other_names` a name or list); a
  built-in name changes only the keys given (`''` clears a built-in
  value; the prompt cannot be cleared). Listed built-ins first, then the
  workspace's (file order): `ListedPlatform { definition, source:
BuiltIn|Changed|Workspace, error }`; a bad pattern stays listed with
  `error`, is a `Problem`, and is not used; structurally bad entries
  (no prompt, unknown key, not a mapping) are `Problem`s, left out.
  `Workspace::platforms`, `platform(text)` (name first, then other
  name, trimmed, ignoring case; `None` if the one found has an error),
  `host_platform(id)` (effective platform), `platform_names()` (lower
  case name/other name → `PlatformName { definition, usable }`, for the
  interface; an other name that finds another definition is a
  `Problem`, as save refuses it),
  `save_platform(previous, def)` (refuses no name/prompt, bad patterns,
  a name or other name another definition has, renaming a built-in;
  built-ins write only differences, none → entry removed),
  `delete_platform` (workspace: delete; changed: reset; built-in:
  refused). `refresh` compares platforms (live reload). Shell:
  `platform_save`, `platform_delete`; `WorkspaceView.platforms` /
  `platform_names`. Interface: `Platforms.svelte`, a view
  tab since #72 (view `platforms`, one tab, no subject; shown only with
  a workspace; kept across a switch, showing the new one's):
  `[data-view=platforms]`, opened by `openView("platforms")` from the
  Hosts view header's / SettingsPanel's `[data-open-platforms]` or the
  palette's `[data-palette-command=platforms]`; Escape leaves the form
  only, `[data-platform-action=close]` closes the tab; rows
  `[data-platform=<name>][data-platform-source]`, `[data-platform-error]`,
  `[data-platform-action=new|edit|delete|reset|save|cancel|close]`, form
  `[data-platform-form]` fields `platform-name|other-names|prompt|
disable-paging|privilege-command|privilege-prompt|show-config|
ignored-lines|file-folder`, `[data-platform-note=file-folder]` (what
  it is for), a row's `[data-platform-file-folder]` ("files in …"),
  `[data-platform-error-message]`). SettingsPanel's
  datalist `platforms` = usable definitions, then names in use that find
  none; `[data-platform-match=<usable definition or "">]` under the
  Device section (also says when the definition found is broken). Tests: `tests/platforms.rs` (fixture `platforms`). Smoke:
  `editInventory` (tab from the details sheet: create (with a file
  folder, checked in the file and the row), change eos (paging and file
  folder)/reset,
  refused pattern; folder platform finding it; tab from the palette
  (Ctrl+K, "platform definitions"): external edit, delete).
  Palette actions (#72, #76): `App.svelte`'s `paletteActions`
  (`PaletteAction { id, label, words, detail, icon, run }`, matched by
  `matchActions` in `CommandPalette.svelte`'s module) are listed after
  the host hits when every typed word is part of their label or words
  (`[data-palette-command=<id>]`, Enter or click runs it; Enter with no
  host hit takes the first action). Ids: `side-<view>` (each side view,
  shown, never folded), `fold-side-view`, `new-host`, `new-folder`,
  `csv-import`, `nornir-import` (the selected folder, as the add
  control), `switch-workspace`, and the view tabs needing no subject
  `preferences|platforms|highlights|python-setup`. Add a new action
  there. Smoke: `paletteActions` (in `searchAndFilter`) runs each from
  the terminal and checks what opened. Jobs use `prompt`,
  `privilege_command`, `privilege_prompt`, `disable_paging` (#43); the
  rest is #45.
- Renaming or moving a host or folder (#99, `src/workspace/moves.rs`,
  `src/history.rs`). `rename_host`/`move_host` → `Moved<HostId>`,
  `rename_folder`/`move_folder` → `Moved<String>`: `{ to, hosts:
[HostMove { from, to }] (tree order; empty when nothing moved), rewritten
(files, paths after the move), not_rewritten: [Problem] }`. After the
  move, every host file and `_folder.yaml` whose own `jump_host` names a
  moved host (trimmed, ignoring case, as `jump_chain` matches) and the
  `_jobs.yaml` entries whose `host` names one (`\` read as `/`) or,
  for a folder (even one with no hosts), whose `folder` is it or below it
  (exactly, as planning finds folders) are rewritten to the new path
  (`jobs::retarget`, bad entries too). Only those
  files are written, by `rewrite_text`: the `key: value` line's scalar is
  replaced in place (indent, comment, line ending kept; the new value
  written plain unless YAML needs quotes), then the result is parsed and
  must equal the expected mapping, else the file is written whole from
  the mapping (comments lost; e.g. a flow mapping). The move itself is
  never undone: a reference that cannot be written is in
  `not_rewritten`. Not rewritten: files that are problems (not loaded),
  saved filters (no paths), kept job results (historical records), host
  keys (by address). History: `history::carry(ws, &moves, &LogFolder,
&ConfigStore, Option<Sessions { manager, records }>)` → `Carried { left:
[LeftHistory { host (new id), from, kind: sessionLogs|configHistory,
folder (where it stays), reason }], sessions (whose records changed) }`
  moves the files (not subfolders: a host and a folder of the
  same name share a path there) of each host's log folder and config
  folder (no workspace id: no configs) to the new path, then removes the
  emptied folders up to `workspaces/<ws>` / `<configs>/<id>`. A new place
  already holding files (an earlier host of that name) → nothing moved,
  reported (never mixed); a file that fails to move → reported, the rest
  moved (the files left in the old place would be found by a later host
  given the old name: the report says where they are); a file created in
  the old folder while moving (a connection that had read it) is swept
  after; case-only renames on a case-insensitive disk rename the host's
  folder only, the folders above keep their case (found either way); a
  new path that cannot hold configurations, with versions kept, is
  reported. Configs move under the store's save lock
  (`while_not_saving`). Sessions: see `sessions.md`.
  With `Sessions`, `SessionRecords::move_hosts(root, &moves)` makes the
  records name the new ids (multi-send pastes, snippet hosts). Shell:
  `host_rename`, `host_move`, `folder_rename`, `folder_move` →
  `Relocated { workspace, result, moved, notRewritten, left }`
  (`relocate`: the edit, then `carry` with the shell's logs, `Configs`,
  manager and records; `announce` if a record changed). Layout:
  `retarget_view` (`window.md`). Interface:
  `inventory.svelte.ts` `relocated()` → `onHostsMoved(listener)` (App:
  session tabs' `hostId` and `profileLogin.host`, config history tabs
  retargeted) and `inventory.moveReport` shown by HostsView
  (`[data-move-report=<new path>]`, items `[data-move-left=<host>]
[data-kind]`, `[data-move-not-rewritten=<file>]`,
  `[data-move-report-dismiss]`; cleared on a workspace switch). Left as
  they are, and why: a session tab's label and header (what it opened
  with, like a role); an open job tab's plan (made before; planning again
  uses the new ids); a kept "Fill in from devices" preview (from the
  kept run, a historical record: the moved host shows as gone, and the
  review comes from the run again); a config history window (its label
  is the old id: it shows no versions until closed and opened again).
  Tests: `tests/host_moves.rs` (references for host/folder rename and
  move, chain of two, flow mapping, nothing moved; history for each, the
  earlier-host clash, an open session, session records),
  `tests/layout.rs` (`retarget_view`), `tests/workspace_edit.rs`. Smoke:
  `jobs` renames `jobs-b1` (logs and three versions) to `jobs-b2` from
  its panel: the files, the history tab and the backups view follow, no
  report. Known limits: a backup already running when its host is renamed
  saves its version under the old path (its job holds the old id), unreported;
  logs are placed by the workspace's folder name, so a second workspace of the
  same name with the same host path shares the folder (#20) and its sessions
  are repointed too; a fallback whole-file rewrite (a quoted key, an
  `answers` variable `host` holding the same id) loses comments; quoted values
  are written plain. Not verified: a failed file move (no test makes one fail).

- Security review (2026-11-22): `PlatformDefinition::check` refuses a
  control character (a line break above all) in `disable_paging`,
  `privilege_command` and `show_config`: a shared workspace could
  otherwise send unseen lines at every login (back up on login). A
  definition with an error is listed but never used (`platform::find`),
  changed built-ins included. Tests in `platforms.rs`. Snippet values
  holding line breaks still give lines of their own (by design: the
  snippet comes from the same workspace).
