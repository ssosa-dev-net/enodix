# Headless binary

`crates/cli/`, the `enodix-cli` binary.

- `crates/cli/` — `enodix-cli`, the headless binary (#48). It has no window
  and needs no running app; like the shell it is a thin adapter over the
  core. Its command line is a contract that the Python bridge builds on:
  `enodix-cli inventory <workspace> [--folder <path> | --filter <name> | --host <id>...]`
  prints `inventory::export` as pretty JSON on stdout. The workspace is
  opened at `app_state::resolved_path`, so `root` is absolute. Each
  workspace problem also goes to stderr as
  `enodix-cli: warning: <file>: <message>`, and the exit status stays 0.
  Exit 1 with `enodix-cli: error: …` for a missing or unreadable
  workspace, or an unknown folder, filter or host. Exit 2 for a bad command
  line (unknown command, an option or value starting with `-` where none
  fits, a missing value, two kinds of limit, no workspace). A folder that
  is not a workspace (#131, `workspace::folder_kind` is `Other`: other
  things, no host file, no `_workspace.yaml`): checked before the fix, it
  was already only read, never written into (`Workspace::open` writes
  nothing; only the app's `prepare_workspace` does), and printed an empty
  inventory with each non-host YAML as a warning and nothing to say why;
  now also `enodix-cli: warning: <folder> is not a workspace: no host
  files and no _workspace.yaml there (nothing was written into it)`,
  exit 0 as before (an empty folder gets no such warning). Test:
  `a_folder_that_is_not_a_workspace_is_read_and_never_written_into`
  (every file and folder compared before and after). `tunnel` and
  `credentials` open alike and write nothing; they do not warn. Also `--help`
  and `--version`. Messages use `CARGO_BIN_NAME`. Values that start with
  `-` (#127; `main.rs` `END_OF_OPTIONS`, `option_and_value`,
  `option_value`, `unknown_option`): `--` ends the options of `inventory`,
  `tunnel`, `credentials` and `secret` (once; after it every argument is a
  value, `--port` too); every option also takes `--option=<value>` (any
  value but an empty one; `--port=` is checked as a port). `--folder -old`
  stays exit 2, its message naming `--folder=<value>`; an unknown option's
  names `--`. Nothing that worked before changed (old binaries refused
  `--` and `=` with exit 2). Tests: `values_that_start_with_a_dash_*`
  (inventory, a workspace `-lab` run from its parent), credentials
  `a_name_that_starts_with_a_dash_comes_after_a_double_dash` (secret `-ro`,
  host `-spare.yaml`), tunnel `it_takes_the_inventory_s_limits` and
  `a_command_line_it_does_not_understand`. Tests:
  `crates/cli/tests/inventory.rs` start the built binary
  (`CARGO_BIN_EXE_enodix-cli`) against the core's fixture workspaces.
  Packaging: `beforeBuildCommand` also runs
  `cargo build --release -p enodix-cli`, so `app:build` and the release
  leave `target/release/enodix-cli.exe` (host target only: a `--target`
  build would need its own). The release config's `bundle.resources`
  installs it beside `enodix.exe`; the generated `installer.nsi` was
  checked to install and uninstall it. The portable zip holds it too. The
  smoke's `headlessInventory` runs it on the smoke workspace first, before
  the app starts.
  Tunnels (#60): `enodix-cli tunnel <ws> [limit] [--port <n>...]`, see
  `tunnels.md`; tests `crates/cli/tests/tunnel.rs`. `--port` (#92), any
  number of times (or `--port=<n>`), for all hosts: digits only, 1 to 65535, else exit 2
  ("--port takes a port from 1 to 65535, not …" / "--port needs a value")
  before anything is opened. The JSON stays `"format": 1`; a listening
  entry gains `"extra_ports": {"<device port>": <loopback port>}`, left
  out when no port was named; `port` is still the SSH port's listener.
  Credentials (#49, `src/credentials.rs`): `enodix-cli credentials <ws>
<host id>` (`\` accepted in ids) prints `HostCredentials`, `enodix-cli
secret <ws> <name>` prints `NamedSecret`, both pretty JSON, exit 1 with
  the reason otherwise. Source: the running app's agent (endpoint of the
  data folder: `ENODIX_DATA_DIR`, else `%APPDATA%` / XDG data / macOS
  Application Support + the identifier; since #128 the settings folder,
  which keeps the vault, the agent's endpoint and the known hosts on
  Windows too: the binary reads nothing of what moved to
  `%LOCALAPPDATA%`, so it works the same on a folder an older version
  left unmoved and never moves anything itself, `release-linux.md`) unless
  it reports `Locked`; else
  the vault file if it exists, its master password asked with `rpassword`
  on the console (prompt `Master password of enodix's vault: `, up to 3
  tries, "Wrong password. …"; a file `Vault::check` finds damaged is said
  to be so first, terminal or not, #125: `a_damaged_vault_is_called_damaged_before_any_password_is_asked`),
  else `KeychainStore` (prefix = identifier,
  or `ENODIX_KEYCHAIN_PREFIX`: the tests' seam). "A terminal" on Windows
  = `GetConsoleWindow()` non-null: a process started with `CREATE_NO_WINDOW`
  has a hidden console rpassword would wait on forever (found the hard
  way); Unix = `/dev/tty` opens. No terminal → "the credentials are in
  enodix's vault, and there is no terminal …: start enodix and unlock it
  | enodix is running but its vault is locked: unlock it there, or run
  this from a terminal". Then (#127, `seen_by_someone`) a Windows console
  must be someone's: a pseudo console (`GetConsoleWindow`'s class
  `PseudoConsoleWindow`: Windows Terminal, an SSH server) or a process on a
  shown window station (`GetProcessWindowStation` + `UOI_FLAGS` has
  `WSF_VISIBLE`; unreadable → asked, as before); else the same refusal with
  "nobody can type the vault's master password here (… as a scheduled task
  does while nobody is logged on)". Found: a task run "whether the user is
  logged on or not" gets a console window on a window station no screen
  shows, so `GetConsoleWindow` alone said yes and rpassword waited on
  `CONIN$` for ever (reproduced by the test below before the fix). stdin
  plays no part (rpassword opens `CONIN$`): run with stdin `NUL`, inherited,
  `CREATE_NO_WINDOW` or `DETACHED_PROCESS` from a process with no console it
  refuses at once; with a new console on the desktop it asks there. Test
  `tests/no_desktop.rs` (Windows, alone in its binary: it switches the
  process's window station for a moment): an unnamed window station (a
  named one is refused to a non-administrator) and desktop of its own, the
  binary started there with `CREATE_NEW_CONSOLE` via `CreateProcessW`
  (`lpDesktop`), 60 s to exit 1 with that message for `credentials` and
  `secret`. Not verified: a real scheduled task (none was made), a pseudo
  console on a hidden window station (sshd) still being asked.
  Never reads a password from args or environment,
  never writes a file. `build.rs` reads `identifier` and `productName`
  from `src-tauri/tauri.conf.json` (`ENODIX_IDENTIFIER`,
  `ENODIX_PRODUCT_NAME`). Tests `crates/cli/tests/credentials.rs`: the
  app = an `AgentServer` in the test process; no terminal =
  `CREATE_NO_WINDOW` (Unix: `setsid`); a terminal = `portable-pty`
  (ConPTY; the test answers `ESC[6n`, strips sequences, checks the typed
  password never shows and the data folder is unchanged); the keychain
  case is Windows-only under a random `enodix-test-*` prefix, removed
  after. Smoke: the `vault` part runs it (`windowsHide`) while locked
  (refused) and after unlocking (gets the profile's password).
  `enodix-cli clear-keychain --yes` (#111, `--yes` since #127): only
  `--yes` (exit 2 otherwise); refused (exit 1) when `ENODIX_JOB_TARGETS`
  is set; without `--yes`, exit 1 "this would remove <n> credential
  profile(s) and <m> named secret(s) from the keychain, of every
  workspace; nothing was removed: run it with --yes to remove them"
  (`KeychainStore::count_all`); with it `KeychainStore::remove_all` under
  the identifier (or `ENODIX_KEYCHAIN_PREFIX`) and prints `removed <n>
  credential profile(s) and <m> named secret(s) from the keychain`; off
  Windows "keeps nothing in a keychain on this system: nothing removed",
  exit 0, either way. Asks neither the running app nor the vault (the vault
  file goes with the data folder). The uninstaller runs it with `--yes`,
  only with the data box ticked. Tests (cli `credentials.rs`, always under
  a random `enodix-test-*` prefix): without `--yes` nothing removed and the
  counts said, then entries under the test prefix and another workspace's
  removed, a longer prefix's kept; arguments; script job.
  `scripts/check-installer.ps1` runs the installed binary's both ways on a
  prefix of its own after the fresh install (not run: it installs). Version details
  (#111): `build.rs` on a Windows target compiles them with `tauri-winres`
  (ProductName, FileDescription "enodix from the command line",
  CompanyName = `bundle.publisher`, the copyright line, version).
