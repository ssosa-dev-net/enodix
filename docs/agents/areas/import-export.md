# Imports and the inventory export

CSV and Nornir imports into a workspace, and the export the headless binary prints.

- CSV import (#26, `src/import.rs`, `src/import/csv.rs`, writer in
  `src/workspace/import.rs`). `import::ImportReport { created, updated,
skipped, failed: Vec<ReportEntry { line, name, host, reason }>,
created_folders, reload_error }` is shared by importers (the Nornir import
  should reuse it and `Workspace::write_imported`, a `pub(crate)` batch
  writer: one reload at the end, new files with `create_new`, re-lists each
  folder on disk so a host created since the preview is `Exists`, never
  overwritten). `import::is_secret_name` = password/secret column names:
  never written whatever the mapping, reported once per column (`line:
None`) when they hold values. `CsvTable::parse/read`: header row, `,` or
  `;` (more of them in the first record), RFC 4180 quotes, UTF-8 BOM,
  UTF-16 with BOM, else UTF-8, else Windows-1252; blank lines dropped;
  short rows padded, extra non-empty values fail the row. `ColumnTarget
{ field | custom | tags | folderPath | ignore }` (serde `kind`),
  `HostField` (file keys), `suggest_mapping(headers)`. `CsvImport { table,
mapping, target }`: `preview(&ws)` (writes nothing) → `PreviewRow {
line, name, outcome: Create|Exists|Skip|Fail, host }` + `new_folders`;
  `run(&mut ws, replace: &[HostId])`. Existing = same folder and the name
  (file stem or `name:`) ignoring case; only ids in `replace` are written,
  taking the row's non-empty values (tags replaced if the row has any),
  the rest kept, name/file unchanged. Folder path: `/` or `\`, matched to
  existing folders ignoring case. Name defaults to the address. Shell:
  `CsvFile` state keeps the table read by `csv_import_read` (→ headers,
  sample, suggested), `csv_import_preview`, `csv_import_run` (→
  `Edited<ImportReport>`). Interface: `CsvImport.svelte`, a view tab since
  #71 (view `csv-import`, no subject: one tab, as the shell keeps one read
  file; Cancel/Close close it, Escape does nothing; the tree beside it
  updates as it writes; a workspace switch closes it)
  (`[data-view=csv-import][data-step=file|map|preview|report]`, input
  `csv-file`, selects `csv-column-<i>`, `csv-custom-<i>`, `csv-target`,
  `[data-csv=read|preview|import|close|cancel]`, rows
  `[data-preview-row=<line>][data-outcome]`, `[data-replace=<id>]`,
  `[data-replace-all]`, `[data-report=created|updated|skipped|failed]`
  with `data-count`), opened by `openCsvImport(folder)` (sets
  `inventory.csvImport`, the target while no preview is shown) via the
  Hosts view's add menu `[data-add=csv][data-open-csv-import]`, FolderDetails' `[data-folder-import]`
  and the folder context menu. Smoke: `csvImport`. Known limits: secrets
  are recognised by name only (since #27 `is_secret_name` also takes
  token, key(s), psk, community, credential(s), bearer, `*apikey`,
  `*privatekey`, `*token`: such columns are left out too; a secret under
  a harmless name is imported); no built-in settings (port, protocol)
  as mapping targets yet. Shared by importers in `import.rs` (#27):
  `PreviewOutcome`, `ImportedHost` (now with `port`, also in
  `ImportedValues`), `FieldValue` (re-exported from `import::csv`),
  `existing_hosts(ws, folder)`, `write_planned(ws, &[Planned], replace,
&mut report)` (the write + report half of every `run`).
- Nornir import (#27, `src/import/nornir.rs`). `NornirInventory::parse(hosts,
Option<groups>, Option<defaults>)` / `read(paths)` (`NornirError::Io |
Yaml | NotAMapping { file: NornirFile }`; empty file = empty; `<<` merge
  keys applied). `NornirImport { inventory, target }` (target must exist,
  no folders created): `preview(&ws)` → `NornirPreview { hosts:
[NornirPreviewHost { line, name, outcome, host }], skipped }`, `run(&mut
ws, replace)`. Resolution as Nornir: host, then its groups depth first
  (each followed by its parents, each once, loops safe), then defaults;
  `data` first-wins per top-level key. `hostname` → address (required:
  no fallback to the name; checked as New host checks it, a `:port` after
  it taken as the port, since #131: below), `port` → `settings.port`, `platform` →
  platform; all extended groups → tags; data → custom fields (nested
  mappings `a.b`, plain lists `, `-joined, other lists JSON; data never
  fills built-in fields). Secret-named keys (`is_secret_name`) are
  removed at any depth before anything else, reported per place
  (`defaults: password`, `group core: connection_options.….secret`,
  `host r1: data.x_password`); `username`, `connection_options` and
  unknown keys are reported as skipped too; a group the groups file lacks
  is a tag plus one note. Lines: the key's line in its file, found by
  scanning column-0 keys (`None` for flow style). Shell:
  `nornir_import_preview { hosts, groups, defaults, target }` (reads the
  files, keeps the `NornirImport` (inventory + target) in `NornirFiles`),
  `nornir_import_run { replace }` (runs exactly what was previewed). Interface: `NornirImport.svelte`, a
  view tab since #71 (view `nornir-import`, one tab, as `CsvImport`)
  (`[data-view=nornir-import][data-step=files|preview|report]`, inputs
  `nornir-hosts|groups|defaults`, select `nornir-target`,
  `[data-nornir=preview|import|close|cancel]`, rows
  `[data-preview-host=<name>][data-outcome]`, `[data-preview-skipped-entry]`,
  `[data-replace=<id>]`, `[data-report=…][data-count]`), opened by
  `openNornirImport(folder)` (`inventory.nornirImport`) via the add menu's `[data-add=nornir][data-open-nornir-import]`,
  FolderDetails' `[data-folder-nornir-import]`, folder context menu.
  Smoke: `csvImport` / `nornirImport` open each from the add menu, then
  from the folder context menu (target checked), check the tree beside
  the open tab (`.slot.view [data-view=…]`) and one tab when opened
  twice; the smoke's `centreOf` now scrolls the element
  into view (the imports made the tree taller than the window).
- First-run words (#116). `import::read_reason(&io::Error)`: a CSV or
  Nornir file that is not there is "there is no such file; check the
  path" (`CsvError::Io`, `NornirError::Io`), not "os error 2".
  `MappingError::NoAddress { first_row_is_a_device }`: says to choose
  Management address for a column; when a header parses as an IP address
  (a file with no row of column names, read with its first device as the
  headers) it says to add a row such as `name,address` at the top. Both
  reports end, when hosts were created, with `[data-report-next]` (a
  double-click connects; a credential profile on their folder or in
  Workspace settings logs them in). Tests: `csv_import.rs`
  (`a_mapping_needs_an_address_column`, `a_csv_file_that_is_not_there_says_so`,
  `a_file_without_a_row_of_column_names_is_told_to_get_one`; since #127
  `a_platform_column_is_suggested_and_gives_the_host_its_platform`:
  headers Platform, OS, Network OS suggested as the platform, written as
  `platform: eos`, the definition then applies, an empty cell sets none),
  `nornir_import.rs`.
- Addresses as New host checks them (#131, `workspace::bad_address`,
  `workspace.md`). CSV (`plan_row`, after "no management address", before
  the name check, so a row named by its address gets this reason): a
  space inside the address fails the row ("the address has a space in it
  (…): write one host name or IP address"); a `host:port` fails it too,
  as a CSV import has no port column to take it into: "write the address
  without the port (…); a CSV import has no port column, so set Port to …
  in the host's details or on its folder". Nornir (`host`, which has a
  `port`): a space fails the host alike; a `hostname` with a port after
  it gives the host that port (`10.1.0.1:2222` → address `10.1.0.1`,
  `port: 2222`) unless its `port` (own, a group's or the defaults')
  differs: "hostname … has port … after it, but port is …: write the
  hostname without the port, or make them the same" (a port above 65535
  after it: "… which is not a port"). Both are reported as `failed` with
  their line, like any row left out; the other rows import. IPv6 is kept.
  "Import N hosts" (`[data-csv=import]`, `[data-nornir=import]`) is
  disabled when N is 0 (nothing new, nothing ticked), with a title saying
  so; so a report with nothing created is no longer reached from the
  preview. Tests: `csv_import.rs`
  `an_address_with_a_space_or_a_port_fails_its_row_as_new_host_refuses_it`,
  `nornir_import.rs`
  `a_hostname_with_a_port_takes_it_as_the_port_and_one_with_a_space_fails`.
  Smoke: `csvImport` (row 7 `10.9.0.6:2222` fails with the port's words,
  not written; the re-import shows "Import 0 hosts" disabled, enabled
  while `odd.yaml` is ticked, then Cancel), `nornirImport` (`spaced`
  fails with the space's words, not written; the re-import's button
  disabled, then Cancel).
- Inventory export (#48, `src/inventory.rs`). `inventory::export(&ws, &Limit)`
  gives an `InventoryExport` with `format` (`FORMAT`, 1), `workspace`
  (root, name, id), `hosts`, `folders` and `problems`. Limits:
  `Limit::All`; `Limit::Folder(path)` (that folder and below; `/` or `\`
  separate, each name trimmed, exact else ignoring case; "" is the root);
  `Limit::Filter(name)` (ignoring case); `Limit::Hosts(ids)` (exact else
  ignoring case, given order, once each). Errors: `ExportError::UnknownFolder`,
  `UnknownFilter { name, known }`, `UnknownHosts(list)`. An `ExportedHost`
  has id, name, address, `folder`, `folders` (top one down to its own,
  root left out), the built-in fields, `tags` (effective), `custom` (a JSON
  object in file order), `withheld` (secret-named custom fields left out,
  as `custom.<key>`, judged by `import::is_secret_name`), `settings`
  (`ExportedSettings`: every effective value, no origins, no web links as
  templates may hold a login; `port` is the setting, else 22/23 by
  protocol, `None` for serial), and `jump_hosts` (connect order: id, name,
  address, port, profile name) or `jump_chain_error` (the `JumpChainError`
  text; the host is still exported). Only SSH hosts get a chain (telnet and
  serial are not tunnelled). `folders` at the top level lists the exported
  hosts' folders with ancestors, root excluded (path, name, description,
  own tags). Hosts are ordered by id, except for `Hosts`. It never reads a
  credential store (profile names only) and never writes (no `ensure_id`).
  `Workspace::find_host_id` and `ResolvedHost::port_or` are `pub(crate)`
  for it. The fixture `export` has a bastion, Madrid (jump host, a telnet
  host that inherits it, a secret-named custom field), Madrid/Lab (a
  missing jump host), a root web link with a login, and `broken.yaml`.
  Tests: `tests/inventory_export.rs`.
