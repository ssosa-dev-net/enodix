# Config backups

The config backup job, config history, back up on login and the history window.

- Config backup and history (#45, `src/configs.rs`, `job.rs`, `job/run.rs`).
  `JobStep::ConfigBackup` (`{kind: configBackup}`): `plan` skips a host
  whose definition has no (blank) `show_config` with
  `SkipReason::NoShowConfig { platform }` (after `NoPlatform`); the plan
  keeps the workspace id privately. `JobOptions` is no longer `Copy`
  (`configs: Option<ConfigStore>`); `check`/`start` refuse a backup with
  `StartError::NoConfigStore` / `NoWorkspaceId`. After the privilege and
  paging steps the engine sends `show_config`; the config = the plain
  output from that point (complete lines only, so the final prompt is
  never in it), minus the first line if it is the prompt shown before
  plus the command (the echo), minus blank lines at both ends (`config_shown`); empty → fails
  "showed nothing"; since #122 a text of nothing but `%` lines (and
  IOS's `^` marker lines) → fails "\"show running-config\" showed only an
  error, not a configuration: \"% Invalid input…\"" (`only_an_error`, a
  second line of defence after the privilege rule of `jobs.md`), so a
  login left unprivileged never keeps its error as a version; since #119
  a configuration whose output went past the job's output limit
  (`JobOptions::output_limit`, 32 MiB, `jobs.md`) fails "... showed more
  than the 32 MiB a job keeps of a host's output; nothing was backed up",
  so no version is ever kept cut short. Since #136 the configuration
  ends only at the very prompt line the show-config command was typed at
  (`Expect::Only`, `jobs.md` "The end of a command"): a configured line
  that ends like a prompt (a banner's `core-7#`, a description) and a
  pause after it no longer end it, and a quiet stop at another
  prompt-like line fails "…, and its last line \"core-7#\" is not the
  prompt \"show running-config\" was typed at (r1#); nothing was backed
  up" after the command timeout. Back up on login reads it the same way
  (`login_backup.rs`
  `a_pause_after_a_configuration_line_like_a_prompt_is_backed_up_whole`).
  Then `ConfigStore::save` → `JobEvent::Backup { host,
outcome: SaveOutcome::Saved | Unchanged { version } }` before `Done`,
  also `HostResult.backup` and `RunHost.backup` (run.yaml, skipped when
  none). `ConfigStore::new(root)` (shell: `DataFolders::configs`, since
  #128 the local data folder's `configs` on Windows, `release-linux.md`):
  `<root>/<workspace id>/<tree path>/<host folder>/<version>.txt` (the
  host folder: the file's stem for `.yaml` files, else the file name,
  since #127, below),
  version = local `YYYY-MM-DD_HH-MM-SS[_n]`; `save(ws, host, text,
ignored: Option<&str>)` compares with the newest version after dropping
  lines the pattern matches (per line, `diff::lines`, so CRLF = LF; blank
  pattern = none), writes temp + rename, one lock per store (clones
  share it); `versions` (newest first, natural `_n`; `lines`, `bytes`),
  `read`, `diff(old, new)` (all lines, ignored ones too), `export(…, to)`
  → `<to>/<stem>_<version>.txt` (`_2`…), `host_folder`. Errors:
  `BadWorkspaceId`, `BadHostId` (a part starting with `.`),
  `NoSuchVersion`, `Pattern`, `NotAFolder`, `Io`. Nothing pruned or
  encrypted. Shell: `Configs` state, `job_start` passes the store,
  `config_versions { host }` (none without an id), `config_version`,
  `config_diff { host, old, new }`, `config_export { host, version,
folder }`. Interface: `openBackup(target, label)` (a job tab of kind `backup`;
  `openJob` with a backup plan sets it too) from FolderDetails'
  `[data-folder-back-up]`, HostDetails' `[data-host-back-up]`, the filter
  bar's `[data-filter-back-up]` and the context menu ("Back up
  config(s)"); the job tab then plans at once and starts at `preflight`
  (`[data-job-backup-note]`: sensitivity), results show
  `[data-job-backup=saved|unchanged][data-version]`; `LiveJob.backups`;
  run lists and tabs title backup runs "Config backup" (`stepTitle`) and show
  `[data-run-backup]`. Config history is a view tab (#70; view
  `configs`, subject = host id, one tab per host, titled "Config
  <name>", closed on a workspace switch like job tabs):
  `ConfigHistory.svelte` (`[data-view=config-history][data-host]`, opened
  by `openConfigHistory(id)` = `openView("configs", id)` from the
  backups view, the details' `[data-open-config-history]` and the
  context menu's `config-history`; no dialog remains): `[data-config-sensitive]`,
  rows `[data-config-version=<id>]`, `[data-config-text]`,
  `[data-config-action=copy|save|diff|back-up]`, selects
  `config-old` (default the second newest) / `config-new`, `[data-diff]`
  rows as in RunView, form `[data-config-export]` input
  `config-export-folder` + Browse…, `[data-config-notice]`,
  `[data-config-empty]`, `[data-config-error]`; refetches on
  `liveJobs.configs` (+1 on every job backup event, job end and backup
  on login: `noteBackup()`, called by `TerminalSession`'s `onBackup`).
  Config backups side view (#70): `ConfigStore::backed_up(ws, &[HostId])`
  → those hosts with versions, in the given order: `BackedUpHost {
host, versions, newest, last_backup (YYYY-MM-DD_HH-MM-SS), last:
LastBackup::First|Changed|Unchanged }` (camelCase); an unchanged save
  writes the note `<host folder>/.unchanged` (its time; a failure to
  write it is ignored), a saved version removes it (not a version: only
  `.txt` files are). Shell `config_backed_up` (the open workspace's hosts
  in tree order; none without an id). `BackupsView.svelte`
  (`[data-backups-view]`, list `[data-backups=n]`, rows
  `[data-backup-host=<id>][data-last][data-versions][data-newest]` open
  the history tab, `[data-last-backup]`; refetches on `liveJobs.configs`
  and the workspace revision; empty: `[data-backups-empty]` explains and
  offers select `backups-folder` + `[data-backups-back-up]` (a folder's
  backup job tab); `[data-backups-error]`). Test device: `show running-config` prints
  `device_config` (`! Command:` line, `CONFIG_TIME` counter line,
  `hostname`, configured lines, `end`) and `configure <line>` adds a
  line (shared per server; `TestSshServer::configure`,
  `running_config(n)`). Tests: `tests/config_backup.rs` (eos ignores the
  time line; #122: `a_backup_whose_privileged_mode_is_not_reached_keeps_nothing`,
  `a_backup_that_shows_only_an_error_keeps_nothing`,
  `a_configuration_shown_slowly_is_kept_whole` (3.2 s of output against
  a 2 s timeout: the timeout counts from the last output)). Smoke `jobs`: a host `jobs-b1` (`platform: eos`, removed
  after) backed up twice from its panel (saved, then unchanged with the
  same version, one file on disk; the backups view's row `first`, then
  `unchanged`; with no configs yet, its empty state's folder backup
  reaches pre-flight), a `Job configure` snippet, a third
  backup saved (row `changed`, 2 versions), the history tab opened from
  the row: two versions, text without the prompt, diff adding the line,
  Save to…, then the details and context menu show that same one tab. Not covered: Copy text in the smoke;
  show-config/ignored-lines values of the built-ins on real devices
  (#47); paged show-config without a disable-paging command; back up on
  login (#46). Known limits: versions are `PlainText` (trailing
  blanks trimmed, lines capped at 4096 columns); the newest version is
  the newest name, so a clock set back (DST fall-back) can compare with
  the wrong one and list out of order.
- Back up on login (#46, `src/job/login.rs`, `session/mod.rs`,
  `session/ssh.rs`). Session side: `SessionManager::open_with(target,
size, OpenOptions { log, when_connected })` (`open_logged` = it with a
  log); `when_connected: Arc<dyn WhenConnected>` is called (from
  `Link::state`) on every `Connected`, reconnects included; duplicates
  keep it. It gets a `ConnectedSession` (`manager()`, `id()`,
  `report_backup(outcome)` = `SessionEvent::Backup(LoginBackupOutcome)`
  on that connection's events, through a weak sender: dropped once that
  connection ended). `SessionManager::open_channel(of, size)` = a second
  shell channel on the connected SSH session `of`'s connection, as a
  session of its own (`Entry.channel_of`; write/ack/close by its id,
  never logged; reconnect/duplicate open another channel): its task
  (`ssh::run_channel`) sends `Command::OpenChannel { size, reply }` to
  the parent, whose loop (`ssh::pump`, now shared by both) opens it one
  at a time (`start_second_shell`, polled inside the loop since
  `Handle` is not `Clone`). Refused (open failure, shell `Failure`, or
  the channel closed instead) → `Closed(Failed(OpenError::ChannelRefused
{ reason }))`. Errors: `SessionError::ChannelsUnsupported` (not SSH),
  `NotConnected`, `NotFound`. Closing a channel closes only it; the
  parent closing ends it. Job side: `LoginBackup::plan(ws, &Profiles, id,
SessionLogin::Profile|Typed, ConfigStore)` → `None` unless the
  effective `backup_on_login` is on, the effective protocol is SSH, a
  usable definition with `show_config` applies and the workspace has an
  id; the host's stored profile's enable secret answers privilege
  whatever the login; `with_command_timeout` (quiet time since the last
  output, as a job's since #122; the 30 min command limit applies). It is the `WhenConnected`:
  spawns `run::back_up_on_login` = the job engine's `HostRunner` with
  `Open::Channel { of }` (`HostWork.target`/`profile` are `Option`s now:
  the fallback target is the profile target for `Profile` (none if a jump host would ask), none for
  `Typed`); a refused channel → second headless connection with the
  profile (host key/login questions refused as in jobs) or, typed, fails
  "...the login was typed...". Outcome `LoginBackupOutcome::Saved |
Unchanged { version } | Failed { reason }` (serde `kind`). Shell:
  `session_open` plans it for `ConnectTarget::Host` (`credentials` some
  = `Typed`) with the `Configs` store; `on_backup` channel on
  open/reconnect/duplicate. Interface: `onBackup` in `SessionHandlers`;
  `TerminalSession` header marker `[data-login-backup=saved|unchanged|
failed][data-version]` ("Backed up" / "Not backed up", reason in the
  title) and, when failed, a quiet `[data-login-backup-failed]` bar
  (`[data-login-backup-dismiss]`); reset on reconnect. SettingsPanel row
  `setting-backup_on_login` with `[data-setting-note=backup_on_login]`
  (not telnet/serial/no definition; sensitivity; since #127 the second
  login, with the stored profile, a device gets when it refuses the second
  channel, so a one-time code or push at login comes twice). Test server:
  `TestSshServer::logins()` (`auth_succeeded`), `refuse_second_channel()`
  (a connection's session channels after the first are rejected).
  Tests: `tests/login_backup.rs` (second channel with one login and the
  terminal answering meanwhile, reconnect unchanged then saved, refused →
  second connection, typed + refused → gives up, store failure event,
  `plan` `None` cases, `open_channel` alone and refused,
  `a_configuration_shown_slowly_is_backed_up_whole` since #122),
  `tests/settings.rs`. Smoke `jobs`: on for `jobs-b1`, connect →
  `unchanged` marker with the third version, `configure` + `exit` in the
  terminal, Enter → `saved` with a new version (3 on disk). A reconnect
  plans it again (#130: the shell's `host_options`), with the host's
  settings and profile of then and the login the reconnect uses. Known
  limits: while the terminal is paused by flow control the connection
  (russh quirk) stalls the backup too; a host key trusted once only does
  not let a fallback connection in; whether real platforms accept a
  second channel is #47's to find out.
- Config history in a real window (#78, redesign spec #61). The history
  tab's `[data-config-action=detach]` "Open in window" →
  `window_detach_configs { host }` opens (or focuses) the window
  labelled `config-<host id as UTF-8 hex>` (a label takes no `.`;
  `windows.ts` `detachedView()` decodes it, `main.ts` mounts
  `Detached.svelte` with `view: { kind: run, run } | { kind: configs,
host }`, `main[data-detached=configs][data-detached-host=<id>]`), and
  the tab closes quietly. There `ConfigHistory` has `detached`:
  `[data-config-action=dock]` "Dock back" (`window_dock` now docks both
  kinds: `dock-configs` (host) → App's `openConfigHistory`); versions,
  text, diff, copy, save to a folder stay; Back up now →
  `window_back_up` (from that window) emits `back-up-host` to the main
  window, which opens the backup's job tab at pre-flight and comes
  forward. Refresh: job backups arrive as `job-event`s in every window;
  a backup on login now makes the shell emit `config-saved` to every
  window too (`forward`'s `Channels.app`), which `startLiveJobs` turns
  into `noteBackup()` (TerminalSession no longer calls it itself).
  Capability `detached.json` covers `config-*`. Closing it closes only
  it; the main window closing, or a workspace switch, closes it as a run
  window. Terminals never detach. Smoke (`jobs`, backup part): detach
  from the tab (tab gone; window lists the versions, text, diff, copy,
  save, back up, no rail/terminal/detach), Back up now there → the
  main window's pre-flight, the backup on login's new version appears
  first in the window, the session menu has no window entry, close the
  window (no tab), reopen, dock back (tab again);
  `closingTheMainWindow` also opens a config history window.
- Renamed or moved hosts (#99). `history::carry` moves a host's version
  files and its `.unchanged` note to its new path under the store's save
  lock, so the history tab and the backups view list them under the new
  id and the next backup (on login too) compares with the newest; an
  earlier host's versions already at the new path are left alone and
  this host's stay at the old one, reported (`LeftHistory`,
  `configHistory`). An open history tab follows (`retargetView`); a
  history window does not. Details in `workspace.md`.
- Hosts whose files share a stem (#127). Until then every host's folder
  was its file's stem, so `r1.yaml` and `r1.yml` (files made outside the
  app; it refuses such a name itself) kept one history: each backup
  compared one device with the other. Now `host_folder` names it by
  `folder_name(file)`: the stem for a file ending exactly in `.yaml` whose
  stem does not itself end in `.yaml`/`.yml` (so every usual host keeps
  its folder of before, nothing moved), else the whole file name
  (`r1.yml/`, `r1.YAML/`, `r1.yml.yaml/`). No two host files of one folder
  share a folder that way (a tree folder of the stem's name, `Lab/` beside
  `Lab.yml`, still holds only folders). Export names keep the stem
  (`r1_<version>.txt`). `ConfigStore::claim_history(&Workspace) ->
Claimed { shared: [SharedHistory { folder, hosts, kept_by, versions }],
left: [(HostId, String)] }`, under the save lock, gives each host whose
  folder changed name the files kept under its stem (versions, the
  `.unchanged` note), file by file (`rename`), then removes the stem's
  folder if empty; nothing is done for a workspace without an id. Only when
  no other host of its folder is or was kept there (stems compared ignoring
  case, then `same_place`: equal paths, or both `canonicalize` to one, so
  `R1/` and `r1/` are one on Windows and two on Linux). **Decision on
  history already shared**: its versions cannot be told apart, so none is
  moved, copied or deleted. If one of the hosts still has that folder
  (`r1.yaml`), it stays that host's history, as it always showed, and the
  others start their own; if none does (`r2.yml` with `r2.YAML`) it stays
  where it is, in no host's history. Each time, it is a `SharedHistory`
  (its `Display` says which hosts and what happens). A claim cut short goes
  on the next time; a version of the same name already in the host's
  folder is never written over (stays, in `left`), a note or temporary
  file there wins over the stem's. Shell: `Shell::open` runs it before
  anything reads history and writes `shared` and `left` to stderr (not in
  the problems list, which reads "could not be loaded"). Not run on a
  reload: a host added outside meanwhile has its own folder anyway; a
  `.yml` host deleted and brought back gets its stem's history at the next
  opening. Tests (`config_backup.rs`):
  `hosts_whose_files_differ_only_in_the_extension_keep_a_history_each` (two
  devices, two backups: each its own version, both unchanged the second
  time), `history_kept_before_for_hosts_that_never_shared_a_name_stays_theirs`
  (a `.yaml` host's folder untouched; `r2.yml` given `r2/` with its note;
  `Lab.YAML` given the files of `Lab/` beside `Lab/r3`; a second claim
  changes nothing), `history_two_hosts_already_shared_stays_where_it_was_and_is_reported`,
  `a_history_claim_cut_short_goes_on_and_writes_over_nothing`. Known
  limits: session logs are still kept per stem (`logging.rs`
  `folder_of`), so the two hosts' logs share a folder (file names carry
  the session's time, so none is written over); an app older than this
  one, run again, does not see a `.yml` host's history in its new folder.
- The terminal's "backup on login failed" bar is in the smoke since #127
  (`jobs`, after the history window's refresh): `Jobs/jobs-b1`'s history
  folder is put aside and a file put in its place, the session ended and
  reconnected: `[data-login-backup=failed]`, the bar
  `[data-login-backup-failed]` with "The backup on login failed: …",
  Dismiss (`[data-login-backup-dismiss]`) closes it; the folder is put
  back.
