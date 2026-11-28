# Testing

The core's integration tests and their support, the containerlab device suite, the example server and the smoke test. Each feature's own tests are named in its area file.

- CI (2026-11-22, PR #1 on GitHub): `.github/workflows/ci.yml` runs on
  pushes to `main` and pull requests, on `ubuntu-24.04` and
  `windows-latest`: `npm run format:check`, `npm run lint`,
  `cargo test --workspace --no-fail-fast`, `npm run test:python`. Actions
  pinned to commit hashes, token read-only, `CARGO_PROFILE_*_DEBUG=0`. About
  11 min on Linux and 19 on Windows. The first runs found four test bugs no
  local machine showed: the Windows runner user is the built-in
  Administrator (SDDL writes `…-500` as `LA`: `canonical_dacl` in
  `credential_agent.rs`); Ubuntu's kernel reports a speed set by number as
  `BOTHER` to `cfgetispeed` (`serial_pty.rs` reads `termios2` in baud);
  `RUNNER~1` in the temp path makes Windows quote a batch file's argument
  (`%~1` in `script_jobs.rs`'s stand-in); the slower runner lost
  `edge_scripts.rs`'s cancel race (cancel on the flood's own output, not
  the test harness's first line). Write new tests with those in mind: no
  string compare of SIDs, speeds or quoted paths, and no cancel on the
  first output of a helper that is the test binary. Later (PR #3): on the
  Windows runner a file's size and time cost as much as reading a small
  file (the open, the scanner), so `edge_runs.rs`'s "listing again is
  quick" ratio is timed off Windows only, and
  `a_settled_description_is_read_again_only_when_its_stamp_changes` checks
  the behaviour itself; prefer such a check to a timing ratio. CI's Rust
  cache restores `target/`, so `target/python-test` is removed before the
  Python tests (a restored venv had pip half there).
  `.github/workflows/dependencies.yml` runs `cargo deny check` (0.20.2,
  `deny.toml`: every exception listed with its reason) and
  `npm audit --omit=dev` on pushes, pull requests and Mondays. Not on CI: the smoke
  test, the containerlab suite, the installer checks (they need a desktop,
  the lab, Windows Sandbox).

- The shell's tests (#70, `src-tauri/src/tests/`, `cargo test -p
  enodix`; part of `npm test` and of CI on Linux and Windows). The app as
  `run` builds it (`with_app`: its state, its main window, every command)
  on Tauri's mock runtime (`tauri` dev-dependency feature `test`): in the
  shell's test build `Runtime`, `AppHandle` and `WebviewWindow` (aliases
  at the top of `lib.rs`) are `MockRuntime`'s, in every other build
  `Wry`'s, so the commands are the app's own, unchanged. `Start` is what
  `run` reads from the environment (data folder, credential store) plus
  the serial ports; a test gives its own (a temp data folder, `memory`,
  `VirtualSerialPorts`), so it sets no variable and touches no keychain.
  `tests/app.rs`: `TestApp::start()` / `with_serial_ports(ports)`;
  `invoke(command, json)` calls a command from the main window through
  the IPC as the page does (arguments by their camelCase names) on a
  thread of its own, and panics if it has not answered within
  `COMMAND_WAIT` (20 s: a deadlock) or ended without answering (it
  panicked); `channel()` is a page's `new Channel()`: the builder's
  `channel_interceptor` routes what the app sends on it to the test.
  `open_session(target)` opens a tab (`layout_edit` `openTab`) and calls
  `session_open` with all eight channels; `TestSession` reads like a
  terminal: `output_showing(text)` acknowledges every chunk
  (`session_acknowledge`), answers ConPTY's cursor query (`ESC [ 6 n`,
  origin `reply`) and matches with CSI and OSC sequences left out;
  `connected()`, `closed()`, `host_key_question()`, `type_text`,
  `close()`; `workspace_answers()` calls `workspace_current`, which waits
  for ever if a command left the workspace held. The test servers are the
  core's (`#[path]` to `crates/core/tests/support/mod.rs`; the shell has
  the core's test dev-dependencies for them). `tests/sessions.rs` opens
  each kind of session that way: a workspace SSH host (host key question
  answered `trustOnce`, logged in with a profile saved by
  `profile_save`, `size` shows the tab's size), telnet (the device's
  `Username: `), serial on a virtual port (its settings opened, both
  ways), the local shell (`ENODIX_WORKSPACE` reaches it) and the system
  ssh tab (the system's `ssh` to a closed port: it says so and the tab's
  session closes; no agent, no known hosts written; needs an `ssh` on the
  `PATH`, as CI's runners have). With #67's fix undone the system ssh test
  fails in 0.1 s (debug builds panic on the second lock, below) or in 20
  s without that check. `tests/workspace_lock.rs` is a lint run as a
  test: it parses every `.rs` file of `src-tauri/src` outside `tests`
  (`syn`, `full` + `visit`; `proc-macro2` `span-locations` for lines) and
  fails on any call, made while `Shell::workspace()` is held, of a
  function that locks it again: the lock is `.workspace()` on `shell`,
  `self` in `impl Shell` or `….state::<Shell>()` (any other receiver but
  `profiles` fails the check: name it `shell`); a function locks if it
  takes the lock or calls one that does (to a fixed point); the lock is
  held from `let guard = shell.workspace();` to its block's end or
  `drop(guard)`, for a whole statement when taken in its temporaries
  (`match`, `if let`, `while let` scrutinees included; a plain `if`'s
  condition only for itself), and inside the closure a helper calls
  holding it (`read`, `edit`, `with_profiles`, `keep_plan`…, found the
  same way); names bound locally are not functions; macro arguments are
  read where they parse. Not followed: a closure kept in a variable and
  called later, a call through a trait object. Its own cases (0.8.0's
  arm caught at its line, the fixed one passing, drops, blocks,
  conditions, closures, helpers of helpers, by name, in a macro, by
  `state::<Shell>()`, an unknown receiver) are beside it. Besides,
  `Shell::workspace` returns a `WorkspaceGuard` that marks its thread
  (`HOLDING_WORKSPACE`); a debug build panics on a second lock on that
  thread instead of hanging (a release build is unchanged), so a test
  that reaches such a call fails at once.
- `tests/` — integration tests through the public API only.
  `output_flow.rs` covers batching and flow control;
  `session_handover.rs` handing a session from one consumer to another
  (`Relay`); `session_relays.rs` the app's relay per session
  (`SessionRelays`); `windows.rs` the layouts per window and the
  app-wide multi-send group; `workspace.rs` loads
  fixture folders from `tests/fixtures/workspaces/` (excluded from
  prettier, as some are deliberately malformed); `workspace_edit.rs` runs
  each edit on a temp copy of the `edit` fixture and checks the files;
  `settings.rs` covers inheritance on the `inherit` fixture (root →
  Europe → Europe/Madrid, each setting something). `search.rs` and
  `filters.rs` use the `search` fixture (its `_filters.yaml` has two
  deliberately bad filters). `credentials.rs` uses `MemoryStore`;
  `keychain.rs` (Windows only) writes real Credential Manager entries
  under a random `enodix-test-<hex>` prefix and deletes them on drop
  (check leftovers with `cargo run -p enodix-core --example
  credential_manager_race -- count`); since #138 it first sweeps
  `enodix-test-*` prefixes untouched for an hour (`support/keychain.rs`,
  also used by the cli's tests: what killed test processes left), and
  plays several processes using the keychain at once (see
  `credentials.md`); since #125 it also
  turns the vault on and off over that real store (an `ActiveStore` over a
  clone of the `KeychainStore`). `support/credentials.rs` `FailingStore`
  also has `Failures.deletes_kept` (a delete says done and keeps the
  entry), `Failures.largest_password` (its `check_fits` refuses with
  `TOO_LARGE`) and `puts_made()`.
  `tests/support/workspaces.rs` has `fixture`, `fixture_copy`, `files`,
  `read`. `tests/support/processes.rs` (#124): helper processes that are
  the test binary itself run again (`helper(role, pids)` = `<exe> --exact
  helper ...` with `ENODIX_TEST_ROLE`; a binary using it has a `#[test]
  fn helper()` calling `play()`, a no-op when run normally): `tree` writes
  `child.pid`, starts a `leaf` (`grandchild.pid`), prints `tree started`;
  both end on their own after a minute. `tree_line()` / `tree_environment`
  for a stand-in interpreter (`FakePython::running`), `tree_python` sets
  the variables itself (a setup step gets no environment); `wait_for_pids`,
  `wait_gone` (polls `running(pid)`: `OpenProcess` + a zero wait on
  Windows, `kill(pid, 0)` and not a zombie elsewhere). A test only ever
  kills a process from its own spawn's handle (the owner helpers), never by
  name or id.
- Edge tests (#117): hostile use of the stored logins and the inventory,
  in files of their own. `edge_vault.rs` (damaged vault files, a write
  cut half way played as the state on disk it leaves, two copies of the
  app as two stores over the same files with their own threads, a reader
  while the app writes; since #125 the lock between copies: two copies
  turning the vault on at once, a save to the keychain or a start while
  another copy turns it on, held there by `support/credentials.rs`
  `Gate` (`FailingStore::gated`: its listing stops at the gate until the
  test opens it), real second processes via `support/processes.rs` (role
  `vault writer`: puts `<copy><n>` once a `go` file is there, prints
  `saved <name>`; two at once, and one killed mid-writes), and on Windows
  a file held without sharing its deletion), `vault_memory.rs` (a global
  allocator that looks at every block freed while the vault is turned on
  and off for a marked secret: its own binary, as it sees every thread),
  `edge_logins.rs` (a locked vault and a keychain
  refusing every read, `support/credentials.rs` `Failures.reads`, met by
  every feature that needs a login), `edge_inventory.rs` (every file kind
  cut at every line and inside each, wrong types, unknown keys, BOM, CRLF,
  tabs, ANSI and UTF-16 text, alias bombs and deep nesting; edits over
  files changed, broken or deleted since they were loaded),
  `edge_inventory_names.rs` (spaces and accents, every name Windows
  reserves, names too long, paths past 260 characters, case-only renames;
  Linux only, `#[cfg(unix)]`: two files differing in case, files with
  names Windows reserves), `edge_inventory_changes.rs` (a pull of hundreds
  of files, a branch switch, an editor saving by rename, a folder renamed
  outside, the workspace folder moved away and back and removed and made
  again, with the watcher), `edge_inventory_scale.rs` (500 against 4 000
  hosts in a flat folder, measured in turn twice, each piece of work under
  24 times as long (linear 8, quadratic 64); the 5 000 and 20 000 measurements
  and the lock held by a refresh of 20 000 (#132)
  are `#[ignore]`d: `cargo test --release -p enodix-core --test
  edge_inventory_scale -- --ignored --nocapture`). Since #132 also: the
  two Unicode forms of a name (names checked free, found, two files side
  by side), deep nesting of every kind refused under 3 s, brackets in text
  not taken for nesting, the folder that cannot be reached or comes back
  as another workspace (`Unavailable`, edits refused), and the watcher
  asked to check again; the tests ignored for #132 run now.
- `tests/support/mod.rs` — the session test harness: an SSH server (russh's
  server side with a scripted shell) started inside the test process, plus
  helpers for waiting on output and state (they acknowledge what they read).
  Its scripted shell has `flood` (endless output until Ctrl+C) and
  `program` / `program off` (a full-screen program that stays: alternate
  screen, bracketed paste, application cursor keys; #84). Docker is not
  assumed. Public-key auth: it accepts a per-server Ed25519 key
  (`write_client_key(path, passphrase)`) and the RSA fixture
  `rsa_client_key()` (`tests/fixtures/keys/`, a throwaway test key;
  `core_folder()` finds the core's fixtures from whichever crate includes
  the harness);
  `client_key()` gives the Ed25519 one. User `OTP_USERNAME` needs
  `PASSWORD` then `ONE_TIME_CODE` (keyboard-interactive; two rounds if
  keyboard-interactive starts it); `start_offering(&[MethodKind])` limits
  the methods offered. `support/agent.rs`: `TestAgent::start(keys)` runs
  russh's agent server in-process on a random named pipe (Unix socket
  elsewhere); `start_as_pageant` listens on Pageant's computed pipe
  (`None`, test skipped, if Pageant runs). Never use the user's real
  agent (`SshAgent::System`) in tests. Login tests: `tests/ssh_login.rs`.
  Each test server has a random Ed25519 host key (`host_key()`;
  `replace_with_new_host_key()` restarts it on the same port with a new
  one). Every server started is trusted in the process-wide
  `support::known_hosts()`; open sessions with `support::manager()` /
  `manager_with_output_options` unless the test is about host keys
  (`tests/host_keys.rs` uses its own `KnownHosts` in a temp folder).
  `next_state` panics on a host-key question; use
  `next_host_key_question`. Every test server forwards `direct-tcpip`
  channels (so it can be a jump host) and counts them
  (`TestSshServer::forwarded()`); its scripted shell ignores data on
  channels that are not sessions (russh passes forwarded data to the
  handler too). Jump tests: `tests/jump_hosts.rs` (temp workspaces
  pointing at test servers). `next_state` skips `SessionEvent::Algorithms`;
  read it with `next_algorithms`. `support/algorithms.rs`:
  `TestSshServer::start_with_algorithms(Preferred)` restricts a server
  (host key type from `preferred.key[0]`: RSA uses the RSA fixture, DSA
  the `dsa_host` fixture; OpenSSH 10 can no longer generate DSA keys),
  `client_offer()` parses what the last client offered (every test
  server records it), `old_device()` is the old-device algorithm set, and
  `FakeDevice::start(Offer)` sends a hand-made KEXINIT with any names
  (e.g. `hmac-md5`), for negotiation failures russh could never serve.
  Legacy tests: `tests/legacy_crypto.rs`. Failure/timeout tests:
  `tests/connection_errors.rs`; reconnect/duplicate: `tests/reconnect.rs`;
  reconnecting a workspace host with its settings of now (#130):
  `tests/host_reconnect.rs`.
  Support: `target_at(host, port)` (a full `SshTarget`), `closed_port()`,
  `SilentServer` (accepts, never speaks), `STALLING_USERNAME` (login never
  answered), `TestSshServer::connections()`, and the shell's `freeze`
  (the connection stops answering, keepalives too). File connections
  (#101): `serve_sftp(folder)` serves a folder over SFTP
  (`support/sftp.rs`: `russh-sftp`'s server half; `/` is the folder and the
  home; listings include `.` and `..`; bare `Failure` for a rename onto an
  existing name, an existing folder, a folder not empty; anything inside
  a folder named `SFTP_LOCKED` is permission denied), `refuse_sftp()` (the
  default), `close_sftp_at_once()` (accept, then exit, EOF and close
  before SFTP's first reply), `close_sftp_and_hang_up()` (the same, then a
  disconnect of the whole connection in the same burst: IOS-XE as the
  lab's CSR 1000v really behaves, #108; the example server's `--scp-only`
  server does this); `shell_requests()` counts shells
  asked for, `connections_alive()` every connection while it lasts; the
  scripted shell's `show sftp` prints how many connections serve SFTP now
  and `cut sftp` disconnects them ("going down"), for the smoke's Files
  tab (#103); `hold sftp <offset>` / `release sftp` hold every SFTP
  transfer from that byte on (`sftp_quirks().hold_from`) and let them go,
  and `scp off` / `scp on` make SCP refuse as IOS before `ip scp server
  enable` (the `AdministrativelyDisabled` quirk) and serve again, for the
  smoke's transfers (#106); SCP (#104): `serve_scp(folder)` answers `scp -t` / `scp -f`
  on an exec channel over a folder (`support/scp.rs`; `/x` and IOS-style
  `flash:x` / `bootflash:x` name the same folder, a fetch's header names
  the file without its folder; any other command, or no `serve_scp`, is
  refused), `scp_quirk(ScpQuirk::...)` adds a device's behaviour
  (`AdministrativelyDisabled`, `PlainText` with `PLAIN_TEXT_REFUSAL`,
  `NoProgram` with `NOT_FOUND` on stderr and exit 127,
  `CloseConnectionAfter`, `WriteFailedAfterData`, `ShortFetch`,
  `StallMidway`: half way through a file it stops reading or sending and
  keeps the channel open), and
  `scp_running()` counts transfers in progress (a send cut short keeps its
  partial file; a fetch streams the file, never reading it whole);
  the transfer engine (#105): `sftp_quirks()` gives `hold_from(offset)`
  (reads and writes from that offset wait until `release()`; SFTP answers
  one request at a time, so everything after waits too: a device that
  stopped answering, while keepalives still are), `keep_at_most(bytes)`
  (writes past it are acknowledged and dropped: a short remote file),
  `refuse_remove()` (permission denied, as a Junos read-only login);
  `sever_connections()` shuts down every accepted socket from a duplicate
  handle (the network going; each duplicate is dropped when its session
  ends, or it would keep a closed connection open). Use it, not `cut_connections()`, while SFTP
  is held: the held server reads nothing, its channel's buffer fills and
  its connection can no longer send the disconnect;
  `HangUpServer` closes each connection before the SSH banner. The server
  keeps a session channel until it asks for a pty or a shell (one that
  asks for `sftp` is handed to the SFTP server, one that runs `scp` to the
  SCP side); a shell's is dropped, as
  an unread channel would block the connection. The unreachable test
  (240.0.0.1) runs on Windows only. Keep the `SessionManager` alive
  for the whole test (dropping it closes the session: `ClosedLocally`).
- Hostile jobs and scripts (#119; the cases and results are in `jobs.md`
  and `python-scripts.md`, "Edge testing"). Files `edge_jobs.rs`,
  `edge_job_scale.rs`, `edge_job_kinds.rs`, `edge_job_agent.rs` (alone in
  its binary: it sets `SSH_AUTH_SOCK`), `edge_runs.rs`, `edge_scripts.rs`;
  `python/tests/test_enrich_edges.py`. `support/jobs.rs`: `Lab` (a
  temporary workspace with an id and the `lab` profile), `host`/`host_at`,
  `follow`/`run` (a job to its report, checking every host once, final,
  one final state, streamed output equal to the report's) → `Ran`,
  `closes` (no session left on a server; a server stalled on purpose in a
  login or shell request keeps its side whatever the client did). Device
  shell lines added: `prompt <name>` (the channel's prompt shows `name`;
  none: the hostname), `crawl <ms> <text>` (the answer and the prompt a
  byte at a time), `stall <ms> <before> <after>` (half a line, a pause,
  the rest and the prompt); `TestSshServer::echo_enable_secret()`,
  `start_device_with_algorithms(hostname, preferred)`;
  `sftp_quirks().full_after(bytes)` (writes past it fail: a full disk).
  Note: `pause <ms>` leaves the shell ignoring input after its prompt (it
  sets `Waiting::Nothing`), so use it as a snippet's last line only, or
  `stall`/`drip`. Heavy cases `#[ignore]`d: `cargo test -p enodix-core
  --test edge_job_scale --test edge_runs --test edge_scripts -- --ignored
  --nocapture` (500 hosts three times, about 3 min; 5 000 kept runs, about
  70 s; 2 × 256 MiB of script output, about 30 s). `#[ignore]`d with the
  finding as reason: a pause after text like the prompt
  (`edge_jobs.rs`), a local file changed during a file job
  (`edge_job_kinds.rs`).
- Hostile devices and links (#118; the cases and results are in
  `sessions.md`, "Edge testing"). `TestSshServer::quirks()`
  (`ServerQuirks`): `misbehave_at(Stage::Login|ChannelOpen|ShellRequest,
  Misbehaviour::Close|Stall|Pause(d))` (`pause_at`), `auth_banner(text)`,
  `output_before_shell(bytes)`, `read_input_slowly(delay)` (per piece of
  input, the whole connection waiting), `refuse_forwarding()`
  (`AdministrativelyProhibited`); `TestSshServer::start_tuned(|config|)`
  changes the russh server config (rekey `limits`, `inactivity_timeout`).
  Shell lines `raw <hex>...` (each word a data message of its own: split
  characters, bytes that are not UTF-8; `support::hex`), `slowly <ms>
  <text>` (a byte at a time), `burst <n>` (a listing and the shell's end
  in one breath). User `THREE_QUESTIONS_USERNAME`: three
  keyboard-interactive rounds. `support/proxy.rs`: `LinkProxy::to_ssh(&server)`
  (trusts the key at its port; `target()`), `stall()` (nothing crosses,
  nothing closes), `resume()`, `cut()` (every connection so far).
  The proxy also stands in front of the telnet test device
  (`LinkProxy::start(port)`, #133). `support/telnet.rs` answers `DO
  TIMING-MARK` with `WILL TIMING-MARK` in order (as IOS and BSD telnetd
  do) and counts them (`TestTelnetServer::probes()`); a `RawDevice` never
  answers one. What the proxy cannot play: a link that drops packets (it
  keeps acknowledging at the TCP level), so TCP keepalive is checked only
  by reading the socket's options back (`session::tcp::tests`, the one
  unit test inside the core; `npm test` runs it with the rest).
  `support/raw.rs`: `RawDevice` speaks raw bytes (`sending(pieces)`,
  `sending_then_closing` with an orderly close, `negotiating_forever`,
  `ping_pong`, `reading_slowly`; `sent()`, `received()`, `refusals()`).
  `support/memory.rs`: `resident()`, `open_handles()` (Windows handles,
  Linux descriptors), `Peak`, `exclusive().await` (one measuring test at a
  time in a file; `transfer_memory.rs` uses it too). Helpers
  `output_until_closed(events, bound)` and `output_of_at_least(events, n)`.
  The heavy cases are `#[ignore]`d: `cargo test -p enodix-core --test
  edge_ssh --test session_resources -- --ignored`.
- Containerlab device suite (#29, `tests/containerlab.rs`, `tests/lab/mod.rs`,
  `lab/`, `docs/containerlab.md`). Opt-in: the `lab_*` tests are
  `#[ignore]`d (so `npm run check` never runs them); `npm run test:lab` runs
  them. The lab file comes from `ENODIX_LAB_CONFIG`, else `lab/lab.yaml`
  (gitignored; `lab/lab.example.yaml` is the committed template); none →
  each prints `containerlab suite SKIPPED ...` and passes; a bad one
  fails. Passwords via `password_env` (default `ENODIX_LAB_PASSWORD`);
  the optional `jump_host` (the clab server) logs in with the agent
  (`ProfileAuth::Agent`) unless given a key file or password: the one
  exception to "never use the user's real agent", as the lab tests are
  opt-in and log in to the user's own server (the suite's non-ignored
  tests never use it). Prompts must stay quiet for `lab::SETTLE` (300 ms)
  to count. `LabRun`
  writes a temp workspace (one host per device, `_folder.yaml` with the
  jump host) and a `MemoryStore`; each `Check` returns an `Outcome`, so
  one device does not hide another's failures. Output is matched after
  `lab::plain` strips control sequences; prompts are regexes at the end.
  The non-ignored tests there run every check against the in-process
  SSH/telnet servers (also via a jump host). Topology
  `lab/enodix-test.clab.yml` (SR Linux, cEOS, FRR from `lab/frr/`) and
  `lab/configs/`. Every check passes against SR Linux 24.10.1, cEOS
  4.36.0.1F and FRR 10.2.1 on the user's containerlab server (2026-11-14;
  IOS-XE and cJunos since #47);
  the platform defaults are the verified ones. cEOS telnet is published on
  the server's port 2323 (`telnet_address` in the lab file, as telnet is
  not tunnelled); cEOS needs `aaa authorization exec default local` for
  privilege 15. The lab file's `jump_host` can use `key_file` when no
  agent runs. The SR Linux large output trickles, so it rarely fills the
  flow-control window; FRR's does.
- Containerlab platform checks (#47). The topology adds `csr1` (IOS-XE
  CSR 1000v, vrnetlab image, 172.20.30.14) and `cjunos1` (cJunos Evolved,
  172.20.30.15); the FRR image has a `vtysh` user whose login shell is
  vtysh (lab platform `vtysh`, device `frr1-vtysh`: what the built-in
  `frr` definition drives; root's shell has no definition). Lab
  platforms `vtysh|ios|nxos|junos` added; each lab platform names its
  built-in `definition` (written as the host's `platform` with
  `backup_on_login: true`; lab key `definition`, `""` = none) and has
  `paging_command`, `privilege_check`/`privilege_expect`, `config_end`
  (+ `enable_secret_env`). Checks `Paging` (snippet job, > 100 lines, no
  pager), `ConfigBackup` (job; last line matches `config_end`; second
  backup `Unchanged`), `Privilege` (only if the definition has a
  privilege command), `SecondChannel` (session with a `LoginBackup`, then
  `SessionManager::open_channel`); tests `lab_paging|config_backup|
privilege|second_channel`. `LabRun` gives its workspace an id and writes
  `_snippets.yaml` (`lab-paging-<dev>`, `lab-privilege-<dev>`); jobs run
  on `trusting_manager` (one session first, as jobs refuse unknown host
  keys). `Conversation::connect_with(OpenOptions)` / `attach` +
  `connected`, `backup(wait)`; backup events are kept aside by
  `event_within`. Results (2026-11-14): every built-in definition but
  NX-OS (no image on the server) passes unchanged; second channel
  accepted by SR Linux, cEOS, FRR, cJunos; **refused by IOS-XE**
  (`ResourceShortage`), so its backup on login uses a second connection.
  Job fix: a config backup drops trailing lines matching the ignored
  lines (SR Linux's two-line prompt left `--{ running }--[  ]--` at the
  end of every kept config); support: `TestSshServer::set_prompt_banner`.
  The enable password question is exercised on cEOS and the CSR 1000v
  since #113: a second login at privilege 1 (`labuser`, with the
  device's enable secret: `lab/configs/ceos1.cfg`,
  `lab/configs/csr1-privilege.cfg`) has lab entries of its own
  (`ceos1-enable`, `csr1-enable`) whose `checks` key names the only
  checks made on them (the job ones; the rest need privilege 15 in a
  terminal). On an entry with an enable secret the privileged mode check
  also requires that the device asked the question, and that a job with a
  wrong secret fails the host as refused. Smoke on a locked workstation: the
  clipboard steps fail (even PowerShell's Get-/Set-Clipboard fail while
  LogonUI runs), so run the smoke unlocked.
- Script lab checks (#93): `tests/lab/scripts.rs` holds what
  `lab_enrichment` and `lab_tunnel_ports` share (`ENODIX_LAB_PYTHON`,
  the cli, `ScriptWorkspace::prepare`: devices behind `jump.yaml`, keys
  trusted by a terminal session, an `AgentServer` over a `MemoryStore`).
  `lab_tunnel_ports` runs `tests/lab/tunnel_ports.py` (prints one JSON
  line per check; the first `junos` and `ceos` devices of the lab file):
  ncclient on 830 (key checked, an Ed25519 stranger key refused) and an
  HTTPS eAPI call with requests on 443. cJunos needs host name, root
  password and `netconf ssh` set by hand after each deploy
  (`docs/containerlab.md`): the topology gives it no config.
- Files lab check (#108): `tests/lab/files.rs`, `Check::Files` (in
  `ALL_CHECKS`, so the suite's own tests run it against the in-process
  servers, which now serve SFTP there), test `lab_files`. Per device: the
  keys trusted by a terminal session (`trusting_manager`), an unattended
  `FileConnection` of `ssh_target` (jump host included),
  `start_files(host_platform)`, then the transfer engine. SFTP: list the
  platform's file folder (else `home()`), clear what an earlier run left,
  upload `files::SIZE` (300 KiB) with the size checked, list it, download
  and compare, rename, delete. SCP after the fall back: send to the file
  folder (`flash:enodix-lab-check.bin` on the CSR), fetch and compare
  (an `ios` device with `flash:` is fetched from `bootflash:`, the CSR's
  other name for the same storage, which the ticket named);
  the file stays (fixed name, replaced each run). `documented(platform)`
  says what `docs/containerlab.md` promises (`ios` SCP; `srlinux`, `ceos`,
  `frr`, `vtysh`, `junos` SFTP; `nxos`, `other` whatever is offered): the
  other protocol fails the check, except the two cases the core has a
  hint for, which are NOT CHECKED with the device's words, the hint and
  the lab config to paste (`Scp::hint` on a Junos fall back →
  `cjunos1-files.set`; `ScpError::hint` on `Administratively disabled.`
  → `csr1-files.cfg`). Suite tests: SFTP leaving nothing behind (and
  clearing an old file), the SCP fall back keeping the one file, both NOT
  CHECKED cases, a device documented with SFTP offering none, a plain-text
  SCP refusal failing in the device's words. Run on the lab on 2026-11-21
  (deployed that day by the agent, `lab/configs/*-files.*` typed in with
  paramiko from the lab server): before the settings, srl1, ceos1, frr1,
  frr1-vtysh ok and csr1, cjunos1 NOT CHECKED with their reasons; after,
  all six entries ok through the jump host.
- Login banner lab check (#134 on real devices): `Check::LoginBanner`
  ("login banner", test `lab_login_banner`, in `ALL_CHECKS`). Lab keys
  `login_banner` and `motd` (text, `|` blocks; `lab::banner_lines` trims
  each line and drops blank ones). A session with `OpenOptions.login_banner`
  `true` must show every banner line, consecutive, before the first
  prompt (`Conversation::wait_for` after `connect_with`; the banner comes
  in with the output kept before `Connected`), and before the motd; one
  with `false` none of the banner's lines that the motd lacks; the motd in
  both. No `login_banner` or `motd`: NOT CHECKED. `config_backup` also
  fails a backup that starts with a banner line or holds a banner line
  more often than the banner does (IOS and EOS keep the text once in
  their `banner` commands). The lab's texts are in `lab/configs/ceos1.cfg`
  (startup config) and `lab/configs/csr1-banner.cfg` (pasted after each
  deploy, like the other csr1 files); the lab file must repeat them (the
  example file does, and a suite test compares it with the configs).
  Suite tests against the in-process servers: `quirks().auth_banner` with
  `output_before_shell` as the motd (passes, also through a jump host), a
  device without the banner (fails), the banner also printed in the shell
  (fails without the option), no keys (not checked), the config backup
  check with a banner on a device-mode server. Run on 2026-11-21 with the
  banners on ceos1 and csr1 (set live, `write memory`, lab not redeployed):
  every check of `npm run test:lab` passed as before (telnet, paging,
  config backup, privilege on both logins, second channel, files);
  `lab_enrichment` and `lab_tunnel_ports` were NOT CHECKED (no Python with
  NAPALM, ncclient and requests on this PC).
- `examples/test_ssh_server.rs` — the same server standalone, for trying the app
  by hand; `--telnet <port>` also runs the telnet test device; `--sftp
<folder>` serves the folder over SFTP (the first and `--device` servers);
  `--scp-only <port> <folder>` runs a server with the same login that
  has no SFTP the way IOS-XE has none (`close_sftp_and_hang_up`, since
  #108) and serves the folder over SCP (the smoke's 22726, #106); its
  `engineer-otp` asks the one-time code at each login, which the smoke's
  `fileTransfers` answers per send in the SCP form (#109; a typed login on
  a host at the workspace's top, `xfer-otp.yaml`; written without being
  run, another worktree held the fixed ports). `--banner <port>` runs a
  server with the same logins that sends an SSH login banner (#134; the
  smoke's 22727, in `agentAndKeyboardInteractive`).
  `--unsupported-device <port>` runs `support::algorithms::FakeDevice`
  (now also `FakeDevice::start_on(port, offer)`) with the MD5-only offer of
  `legacy_crypto.rs`: it sends its KEXINIT and nothing more, so the app's
  failure card is `unsupported` and offers the system's ssh client, which
  hangs in its key exchange (the smoke's 22728, in `screensOf080`).
- `scripts/linux-desktop-check.mjs` (`npm run test:linux-desktop`, #114) —
  drives the installed Linux app (or an AppImage) over WebDriver
  (`tauri-driver` + `WebKitWebDriver`) through docs/linux.md's desktop
  check list, under WSLg or on a Linux desktop; much smaller than the
  smoke test. Details in `release-linux.md`.
- `scripts/smoke.mjs` — end-to-end smoke test that drives the built app.
  **Where to run it.** `npm run smoke` puts the app's windows on the
  desktop it is started from and takes the keyboard focus: on a computer
  someone is using it interrupts them, and their own typing and clicking
  fails it at random steps (a menu that closed, a session window that never
  shows connected; several such failures on 2026-11-21 were this, not
  defects). `npm run smoke:hidden` (`scripts/run-on-desktop.py`, Windows,
  needs `py`) runs the same test on a Windows desktop of its own
  (`CreateDesktop` in the user's window station, the process started with
  that desktop): nothing is on the user's screen and nothing takes their
  focus; rendering (WebGL), focus inside the app, copy and paste, session
  windows and pictures all work there (303 checks passed that way on
  2026-11-21). The log is `target/smoke-hidden.log`, printed at the end.
  Still shared with the user's session: the clipboard (a copy made by the
  user while a clipboard step runs can fail that step, and the test's text
  is on their clipboard for a moment), Credential Manager and the Windows
  account. One smoke run at a time: its ports are fixed. Agents working
  side by side take turns with a lock folder
  (`%TEMP%\enodix-app-driving.lock`, made with `mkdir`, removed when the
  run ends; one older than 40 minutes was left behind and may be removed).
  A new smoke step is unproven until the smoke test has run once: two steps
  written without a run on 2026-11-21 both failed for reasons of their own
  (a button clicked before it was drawn, a host-key question of a new port
  not answered). The third place to run it, with nothing shared with the
  user at all, is Windows Sandbox (below).
- Windows Sandbox (#139, `scripts/sandbox.mjs`, `scripts/sandbox-inside.ps1`,
  `scripts/sandbox-installer.ps1`). `npm run smoke:sandbox [-- --no-network]
  [-- --vgpu]` and `npm run test:installer:sandbox -- --old <setup.exe>
  [--new <setup.exe>]`. Needs the Windows Sandbox feature (turned on once,
  elevated, with a restart) and its `wsb` command line, which arrives with
  the sandbox's own update the first time the sandbox is started (on
  2026-11-21 the first `WindowsSandbox.exe` start showed "Downloading and
  installing updates..." and ended; after it `wsb.exe` was in
  `%LOCALAPPDATA%\Microsoft\WindowsApps`, sandbox app 0.8.114.0). Before
  the smoke: `npm run app:build` and `cargo build -p enodix-core --example
  test_ssh_server`; before the installer check, a release build.
  **How it runs.** The host writes `target/sandbox/<mode>-<time>/`:
  `tree/` (a copy of what the tests read from the tree: `scripts/`,
  `crates/core/tests/fixtures/keys/`, `src-tauri/tauri.conf.json`,
  `Cargo.toml`), `out/job.json` and the configuration, takes the
  app-driving lock (waits for it), refuses if a sandbox runs already (one
  at a time), and starts it with `wsb start --config <xml>`. Nobody is
  logged on in a sandbox until a remote session connects (its logon command
  does not run, `wsb exec --run-as ExistingLogin` fails), so `wsb connect`
  is started through `scripts/run-on-desktop.py` on the desktop
  `enodix-sandbox`: the session's window is never on the user's screen
  and nothing takes their keyboard. Mapped read-only: `tree/`, the app's
  folder (`target/release`), the test server's folder
  (`target/debug/examples`), this Node's folder and a Python's folder
  (`py -3`'s `sys.base_prefix`, or `SANDBOX_PYTHON`): mapped as they are,
  no download (python.org's, the install manager's or uv's Pythons run from
  any folder; 3.15.1 from the install manager was used); writable: `out/`.
  The logon command (`sandbox-inside.ps1`, run as `WDAGUtilityAccount`, an
  administrator, elevated, no UAC) copies the app and the server to
  `C:\enodix` (the smoke writes beside the app), puts the fixture keys at
  the absolute path the test server was built to read them from (its
  `CARGO_MANIFEST_DIR`, on a `subst` drive if the sandbox has no such
  drive), gives the debug-built server `vcruntime140.dll` from the mapped
  Python (a clean Windows has none; only the test's server gets it), puts
  Node and Python on `PATH` and runs `node scripts/smoke.mjs` with
  `SMOKE_APP_FOLDER`, `SMOKE_TEST_SERVER`, `SMOKE_PYTHON`,
  `SMOKE_SCREENSHOTS`, `SMOKE_NET_LOGS` and `SMOKE_WEBVIEW2_POLICY` (none
  set, the smoke test is as before). It writes `out/sandbox.log` (followed
  on the host as it grows), `smoke.log`, `pictures/`, `net-logs/` and last
  `done.json` with the exit code, then shuts the sandbox down, which ends
  it; the host stops it with `wsb stop` if it did not and ends the session
  window left on the hidden desktop. The sandbox has its own clipboard
  (`ClipboardRedirection` off), audio, video and printers off, 8 GB.
  **What the sandbox has (2026-11-21):** Windows
  11 Enterprise 26100, WebView2 157.0.4341.60 (the host's), no VC++
  runtime, no Python (no `py`, no Store aliases for `python`/`python3`),
  no Microsoft account and no OneAuth folder, a NAT address on the Hyper-V
  default switch, the "Microsoft Remote Display Adapter".
  **Found there:** (1) `enodix-cli.exe` did not start on a clean Windows
  (0xC0000135, no `vcruntime140.dll`): the installer's cli, script jobs and
  the uninstaller's `clear-keychain` would all have failed where no other
  program had installed the VC++ runtime; the cli now links it statically
  as Tauri does for the app (`crates/cli/build.rs`, `static_vcruntime`).
  The app's `enodix.exe` already did. (2) WebView2 150 and later ignores
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` (and the HKCU policy) in an
  elevated process, and everything in the sandbox is elevated: no DevTools
  port, the smoke could not reach the page. `SMOKE_WEBVIEW2_POLICY` makes
  the smoke also write its arguments to
  `HKLM\Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments`
  (value `enodix.exe`), which works there (needs administrator rights:
  only inside the sandbox). (3) With vGPU on, the sandbox's `dwm.exe`
  crashed in a loop (`udwm.dll`, 0xc00001ad) and the session was lost
  ("The connection to the Windows Sandbox environment was lost") with its
  window on the hidden desktop; with vGPU off it works, so vGPU is off
  unless `--vgpu` (not tried with the window on a visible desktop). With
  vGPU off the app still renders with WebGL (software): the terminal's
  WebGL context-loss step passes. (4) The Jobs view's cancel was checked
  at once after the view opened and failed there; the step now waits for
  it.
  **Results (2026-11-21).** `npm run smoke:sandbox` passed, all 303
  checks, network on (401 s for the test, 430 s in all: the sandbox starts
  in 2 to 3 s and its logon command runs about 10 s later) and with
  `--no-network` (387 s, 408 s in all; the app needs no network for
  anything the smoke checks). The three runs before failed each at a
  different step (a typed line in a session window's split pane not
  echoed in time, the Jobs view's cancel, a side-panel field set before it
  was drawn): the sandbox runs the test about 1.6 times slower than the
  host (software rendering on a 2736 × 1111 remote display), which shows
  races the host hides; the last two now wait, the first was not seen
  again. Clipboard steps pass (the sandbox's own clipboard); focus inside
  the app works as on a hidden desktop; the connection watch needs no
  administrator rights there either. **The connection watch, network on,
  with no Microsoft account on that Windows:** 180 connections, every one
  to the test or between the app's own processes on loopback, 0 to
  Office, 0 to the sign-in service, 0 UDP sockets, and WebView2's network
  logs looked up and requested nothing beyond the computer (with the
  network off: 174 and the same). So the #135 connections need a
  Microsoft account signed in to Windows (no OneAuth folder ever appeared
  there). `npm run test:installer:sandbox -- --old <0.4.1's published
  setup.exe>` (new: this tree's `--unsigned` build) passed: about 75 s in
  the sandbox (`check-installer.ps1` 37 s); the installed app showed its
  window 1 s after its start; the uninstall through the window with the
  data box ticked left nothing of the app's (install folder, uninstall
  entry, vendor key, shortcuts, both data folders, both Credential
  Manager entries all gone; the other program's entry stayed); NSIS's
  `~nsu1.tmp` stays in `%TEMP%`. **Not verified there:** vGPU on with the
  window on a visible desktop; the app's own first-run walk of #116 with
  the installed app (it is only started and closed); an update through the
  updater (an unsigned build has none); the sandbox runs Windows 11
  Enterprise 26100, so a Home or Pro edition is
  not what is checked.
  **It clicks with the mouse, as a user does (#94).** `mouseClick(evaluate,
selector, { button, double, mayClose })`, `mouseClickElement(evaluate,
expression, what)` (an element found by its text) and
  `clickButton(evaluate, label, scope)` (a button by its label) move the
  mouse there first (controls shown on hover appear), check that the
  control is the top element at the centre of its visible part (cut by
  clipping ancestors) and stays put, then send real mouse events over the
  page's own DevTools client (`evaluateIn` sets `evaluate.devtools`; the
  session, run and config windows each have theirs). Right-clicks,
  double-clicks and menu entries go the same way. A control something is
  drawn over fails the run: `a click on X would land on Y`. No
  `.click()` is left (the #94 bypass for known findings went with #100).
  A pane's tools are used through `paneTool(evaluate, bar, tool, what)`:
  the button, or the "…" menu when the bar has folded them;
  `foldedToolEntries` lists that menu's entries. Not clicks, and kept:
  forms submitted with `requestSubmit()` (Enter in a field), fields filled
  through `value` and input events, a terminal focused with `.focus()` (a
  session window's after a blur: DevTools never moves the OS focus, so the
  window coming forward is played that way). A real click moves focus
  as a user's does, so a step that types after clicking a control first
  waits for the terminal to have the keys. Run time (app built, whole
  run): 162 s before #94, 199 s after. With the terminal's `isolation:
isolate` taken out, the run fails on the host-key question's Trust once
  (it lands on xterm's scrollable element). The
  first quick-connects trust the host key once, then remember it (in the
  smoke's temp data folder); it then writes another key into `known_hosts`
  to check the changed-key warning, Cancel, and replacing. Its
  `credentialProfiles` part creates, assigns, edits and deletes profiles; its
  `searchAndFilter` part drives the palette and a saved filter; its
  `agentAndKeyboardInteractive` part answers a one-time code and uses
  agent profiles (the example server's `--agent <pipe>`, reached through
  `SSH_AUTH_SOCK` set for the app); its `legacyCrypto` part connects to
  the example server's `--old-device <port>` (22723) with the switch off
  (failure card), then on (header marker); its `screensOf080` part (after
  `legacyCrypto`) drives what 0.8.0 added: quick connect's Local shell
  mode (the shell list, Command Prompt's prompt in the workspace folder, a
  command's output, the Local marker, no log file, no multi-send entry)
  and the palette's New local shell (PowerShell); a Lab host with a local
  forward to a TCP server of the test's own on `FORWARD_PORT` (22729) and
  `D 0` (**Forwarding 2**, the list, a connection through the forward and
  one through SOCKS, **Stop**, both closed with the session); a host with
  `agent_forwarding: true` and the `agent-login` profile (the question
  before the login, Escape, Forward for this session over a reconnect,
  Always and `agent_forwarding.json`, the device's `agent` line); a host
  with `auto_reconnect: true` (`drop`, the countdown, the try made by
  itself, a key stopping the next, Enter); Preferences' key bindings (a
  moved shortcut, a refused key, a snippet key sending once) and the
  Dracula colour scheme read from the xterm theme; and the
  `--unsupported-device` stand-in's card, **Open with the system ssh
  (insecure)** and the **Insecure** tab. It leaves nothing behind (hosts,
  snippets file, preferences reset). The serial file transfer is not in
  it: the smoke test has no serial port. With it (2026-11-27,
  `npm run smoke:hidden`) the run is 341 checks (11 of them this part's)
  and 412 s, app built. Its first runs found two defects of 0.8.0,
  fixed with it: the system ssh deadlock (`sessions.md`) and a local
  shell whose terminal stayed empty when opened at once after the page
  loaded (`sessions.md`, Local shell). Its `telnet` part double-clicks
  a `protocol: telnet` host (the example server's `--telnet` 22724), logs
  in in the terminal, checks `[data-unencrypted]`, and uses the side
  panel's `[data-connect=telnet]` on an SSH host; its `serial` part
  checks the quick-connect port list, refresh and 9600 8N1 defaults, then
  opens the missing COM250 (quick and from a `protocol: serial` host) and
  checks the failure names it; its `tabsAndPanes` part (last) closes every
  tab, opens sessions in tabs, switches/moves them by keyboard and drag,
  checks held output in a hidden tab, splits right and down, drags a tab
  into a pane, forces a WebGL context loss, confirms closing, closes a pane
  and opens twenty sessions; its
  `liveReload` part changes files on disk and checks the tree and panel,
  then removes the workspace folder and makes it again (#132: the
  "cannot be reached" notice, then a host added after shows);
  the first-run words of #116 (advice under a failed connection, the
  welcome and switcher refusals, New host's refusals, the import reports'
  next step, the credential hints) are checked where those screens are
  already driven (list in `window.md`); its
  `editInventory` part edits the tree and side panel (real mouse events for
  drag and the context menu) and checks the workspace files on disk.
- The whole-run connection watch (#115, `scripts/connections.mjs`,
  `scripts/watch-connections.ps1`). Before the first app starts (after the
  release and test servers), `watchConnections({ ports })` starts a
  PowerShell that compiles a small C# sampler (Add-Type, ~2 s, no admin
  rights) and every 50 ms reads the process list (Toolhelp snapshot) and
  the system's TCP tables (IPv4 and IPv6) and UDP tables with owning pids
  (`GetExtendedTcpTable` / `GetExtendedUdpTable`). Watched: every
  `enodix.exe` and `enodix-cli.exe` whose parent is the smoke's node, and
  every process they start, transitively (by parent pid, a child created
  before its parent refused as a reused pid, and so is one whose start
  time this user cannot ask: a system process such as `wininit.exe`,
  whose recorded parent ended at boot and whose parent id a process of the
  app got later, once made a run blame a Windows service's UDP socket on
  the app): WebView2's
  `msedgewebview2.exe` (browser, renderers, GPU, network service), the
  Python the app runs (`py`, the venv's launcher and the real
  `python.exe`) and the `enodix-cli` a script runs. It prints each new
  (pid, state, local, remote) once, as a JSON line; for a loopback remote
  it looks up the remote end's owner (the other end's row, else the
  listening socket) and says whether it is watched or the test's (node or
  another of its descendants: the example server, the release server).
  Judged in node as each line arrives: allowed are a listening socket on
  loopback, a connection to loopback owned by the test or the app, or to a
  loopback port the test named (`CLOSED_PORT` 1, for the refused
  connection, has no owner); anything else (any non-loopback remote, a
  listener on `0.0.0.0`, a UDP socket on a non-loopback address) is a
  problem with the step that was running. Three things of WebView2's own
  are known and handled by name:
  - Its sign-in with the Windows account (#135): the browser process to
    port 443 of 40.96.0.0/13 or 52.96.0.0/14 (OneAuth fetching the
    account's profile from `substrate.office.com`, a few seconds after a
    start, not caused by typing). The app starts WebView2 with that
    sign-in off (`WEBVIEW_ARGUMENTS`); until 2026-11-22 the watch let the
    connection through and listed it, since then it fails the run, with
    the reason in the message (`officeConnection`). Measured on this
    computer, signed in to a Microsoft account: 14 runs with the switch
    (8 whole, 6 stopped part-way by other failures) made none; 4 whole
    runs without it made 6, 9, 6 and 9, two of them straight after runs
    with it.
  - Its IPv6 check: the network process (a child of the browser process)
    points a UDP socket at a fixed Microsoft address and sends nothing;
    passed and counted (`probes`), and `netLogProblems` fails on any
    datagram sent.
  - What it lets through of the app's own addresses (#142). The pages are
    at `https://tauri.localhost/` and commands go to
    `https://ipc.localhost/`; WebView2 answers both itself, but it opens a
    connection ahead of time for each new page (a preconnect), and a
    command in flight when its window closes or reloads is let through to
    the network once the window is gone. On `http://` (every release up to
    0.4.1) that put commands on port 80 of this computer in clear: 16 in
    one run, about 79 KB, all from closing or reloading windows
    (`session_dock`, `session_dock_all`, `window_dock`,
    `plugin:window|close`, `window_open_session`, `python_status`,
    `main_window_close`). The shell now builds every window with
    `use_https_scheme` (`HTTPS_SCHEME` in `src-tauri/src/lib.rs`). The
    test listens in place of another program on 127.0.0.1 and ::1
    (`listenOnThePagePorts`): on port 80 a plain listener, where any byte
    fails; on port 443 a web server with a self-signed certificate for
    those names (`scripts/fixtures/page-port-*.pem`, made for the test
    and worth nothing), where a completed TLS handshake or a request
    fails. Checked at every step (`nothingAskedOfThePagePorts`). First
    run on https: port 80, 0 connections; port 443, 92 connections, no
    handshake completed, no request. `SMOKE_PAGE_PORT_REPORT=1` lists
    what arrives instead of failing (first line and host of each request,
    never a body) and hangs up as a closed port would. Where a port is
    not the test's to take, connections to it are let through unjudged
    and the last step says so.

  Every `step()` in `main` calls
  `checkConnections()` first, so the run fails at the next step, with the
  process lineage (`msedgewebview2.exe 14864 < enodix.exe 53236`).
  Second source: every app start passes WebView2 `--log-net-log=<file>
  --net-log-capture-mode=Default` (`netLogArguments`), the same file for
  every copy: with a file per copy the restart after "Close all" failed
  (the old browser process was still ending, and WebView2 refuses other
  arguments for its data folder), so `harvestNetLog` reads the last
  copy's log before the next copy starts (`SMOKE_NET_LOGS=<folder>` keeps
  the last one); `netLogProblems(file)` reads it line by line
  (a log cut short by the kill still counts) and lists every name looked
  up and every http(s)/ws(s) URL requested whose host is not `localhost`,
  `*.localhost` or loopback. `noConnectionOfItsOwn` (the last part) ends
  every app, waits 2 s, checks once more and checks every net log; a
  failing `checkConnections` adds what the net logs show so far. Cost:
  ~2 s to start the sampler and 2 s at the end; the whole run took 243 s
  on 2026-11-21 (199 s was noted after #94, on a quieter machine; the
  sampler's own CPU was not measured). That run watched enodix.exe ×7,
  msedgewebview2.exe ×63, py.exe ×2, python.exe ×71, conhost.exe ×37,
  enodix-cli.exe ×3, taskkill.exe ×1 and, unexplained, rustup.exe and
  rustc.exe ×2 each (most likely another worktree's build reusing a pid
  of a watched process that had just ended; they made no connection):
  162 connections, 0 UDP sockets, 8 Office connections (then still let
  through).
  **What it cannot see:** a TCP connection opened and closed within one
  50 ms sample (Windows keeps a refused SYN to loopback in SYN_SENT for
  about two seconds, and a remote handshake plus TLS takes longer than a
  sample, so ordinary connections are seen; a connection to a host that
  answers within milliseconds and is closed at once could be missed);
  TIME_WAIT rows (no owner pid); where a UDP datagram goes (only the bound
  local address is in the table, and a socket bound to `0.0.0.0` counts as
  a problem); name lookups by the app's own processes (Windows does them
  in the DNS Client service, `svchost`, and its cache cannot be read
  without elevation here), except WebView2's network-stack lookups, which
  its net log shows; anything a process does through another process
  (BITS, a Windows service, the browser a web link opens); and processes
  started outside the tree (an installed enodix, the uninstaller).
  Proved to fail: with WebView2 on wry's arguments only, the same watch
  (a dev script on its own ports) failed at once on
  `config.edge.skype.com` (a client id, OS version and install date in
  the URL) and `wpad` lookups; the smoke then failed on
  `edge.microsoft.com` component updates a minute in and on Office
  servers (then put down to typing; #135 found it is the Windows
  account's sign-in, see `window.md`); the smoke's own
  refused connection to `127.0.0.1:1` failed it until `CLOSED_PORT` was
  named; a temporary thread in the shell that
  connected to `192.0.2.1:443` 30 s after start failed the smoke with
  `enodix.exe (pid …) connected to 192.0.2.1:443 (SYN_SENT), during the
  step after "a failed negotiation listed …"` (2026-11-21, then removed).
  The connections WebView2 makes outside its network stack (the Office
  ones, from the browser process) are not in its net log: only the TCP
  sampler sees them.
  The #135 reproduction (not in the repo; how to rebuild it): start
  `target/release/enodix.exe` from node with the smoke's environment
  but a new `ENODIX_DATA_DIR` and `WEBVIEW2_USER_DATA_FOLDER` per start,
  DevTools on 9411, run `watch-connections.ps1 -Root <node pid>` beside
  it, click `input[name=host]` of the quick-connect form and type (CDP
  `Input.dispatchKeyEvent`) or stay idle for 30 to 45 s, kill the tree,
  count distinct local ports to non-loopback addresses per start. Compare
  variants interleaved (switch first, control right after): the rate
  drifts with OneAuth's shared cache (`window.md`), so batches run at
  different times are not comparable. A variant's `--disable-features`
  goes in `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` and replaces the app's
  list, so it must repeat it.

- Windows Sandbox and Smart App Control (2026-11-22, PR #8): the
  sandbox's own Windows (11 Enterprise 26100, not the host's) switched
  Smart App Control on during the evening (off at 19:11, on by 23:10, no
  host change) and then refused every unsigned program: "An Application
  Control policy has blocked this file", even the published installer.
  `sandbox-inside.ps1` `SmartAppControlOff` sets
  `VerifiedAndReputablePolicyState` 0 and runs `CiTool --refresh` at
  logon (the sandbox is thrown away; the host is untouched); `Facts` logs
  the state and the enforced policies, and a failed run logs the latest
  code-integrity events (`CodeIntegrity`).
