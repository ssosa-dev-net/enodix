# Sessions and transports

The core's session manager and its transports (`crates/core/src/session/`), SSH logins, host keys, algorithms, jump hosts, telnet, serial and session logging.

- `src/session/` — the session manager and the one session interface (bytes in,
  bytes out, resize, close, state changes). Each transport (`ssh.rs`,
  `telnet.rs`, `serial.rs`) is a function that drives a `Link` and returns a
  `CloseReason`. `tcp.rs` has `tcp::connect` (resolve, per-address timeout
  share, failure kinds), shared by SSH and telnet.
- `src/session/output.rs` — the output pipeline: `SessionEvents::next` batches
  output (idle flush, else 10 ms / 64 KiB), and flow control (`Flow`) counts
  unacknowledged bytes. Consumers **must** acknowledge output
  (`SessionEvents::acknowledge` or `SessionManager::acknowledge`, or
  `Relay::acknowledge` with the consumer key once a relay owns the events,
  as in the app), or reading
  stops after `OutputOptions::window` (256 KiB). A transport only reads from
  the device inside `flow.room_to_read()` and keeps taking commands meanwhile
  (see `ssh.rs`); telnet/serial do the same.
- russh quirk: it adjusts the SSH window by itself; when we stop reading its
  channel, its 100-message buffer fills and its whole connection loop blocks,
  including outgoing data and keepalives. So while the interface is stalled,
  writes queue until acks resume, and a paused session does not notice a
  remote close or a dead link (why it cannot be helped in the session code:
  "Links that go silent", below).
- `src/workspace/jumps.rs` — `Workspace::jump_chain(&ResolvedHost)` →
  the jump hosts in connect order (first = reached directly), each
  resolved with its own settings. References match host ids, then
  ignoring case. A host whose _inherited_ jump host is itself (the
  site bastion in the folder naming it) connects directly; naming itself
  in its own file is a loop. `JumpChainError::Missing | Loop | NotSsh`.
  `Missing.named` (#116): the one host whose _name_ is the reference (a
  newcomer writes `bastion`, not `Site/bastion.yaml`); the message then
  gives its id. References still match ids only (the setting's list
  offers ids, and its note says so).
  `Profiles::jump_hosts(id)` turns the chain into `SshTarget.jump_hosts`
  (`JumpHost { name = host id, host, port, username, auth }`); a jump
  host without a usable profile (none, or not set up here) gets
  `SshAuth::Prompt` and an empty username: once its host key is
  trusted, the session asks "Username: " (echo) and "Password: "
  (hidden) as a `SessionEvent::Prompt`, answered with
  `SessionManager::answer` (the interface's `LoginQuestions`, no
  interface change). `SshAuth::Prompt` with a username asks only the
  password. In `ssh.rs`, each hop is reached through the one before with a
  `direct-tcpip` channel + `client::connect_stream`, with its own
  host-key check (questions name its address/port) and login; its
  questions (keyboard-interactive or the prompted login) get
  `LoginPrompt.name` "Jump host <id>" (`Prompter.jump_host`).
  A hop's failure is `OpenError::JumpHost { name, error }`; a device the
  last jump host cannot reach is `OpenError::Connect("the jump host X
could not reach it ...")`. The jump connections live in `Shell.jumps`
  and are closed after the device. Each hop has its own connect timeout
  (`JumpHost.connect_timeout`, from its own settings). The interface
  learns about a broken chain only from `session_open`'s error (shown
  as the closed reason), after any username/password prompt.
- `src/session/agent.rs` — SSH agents. `SshAgent::System` = the named
  pipe in `SSH_AUTH_SOCK` if set (on Windows only a `\\.\pipe\…` value
  counts; Git Bash sets Unix paths), else the Windows OpenSSH pipe then
  Pageant (keys of every one running); `At(path)` = one pipe/socket;
  `Pageant` = the `pageant` crate (its named pipe, else the window).
  Agents are reached and their keys listed **before** connecting
  (failures: `OpenError::Agent(AgentError::NotRunning|NoKeys|
KeysRefused|Failed)`; `CloseReason::offers_password_login()` is true for
  these and the interface then offers a password prompt). Certificates
  in an agent are ignored.
- Login flow (`ssh.rs` `authenticate`): `none` first to learn the
  methods; then the target's method; then keyboard-interactive only if
  offered and (partial success, or password method not offered, or a key
  file refused). After a key or agent key accepted as a first step
  (partial success) with `password` offered next and keyboard-interactive
  not logging in, "Password: " (hidden) is asked (`ask_password`) and sent
  with the password method; a further partial success goes on to
  keyboard-interactive (issue #26, item 8). Test server user
  `KEY_THEN_PASSWORD_USERNAME`; tests in `ssh_login.rs`
  (`a_server_that_wants_a_password_after_the_key_asks_for_it` and the
  three after it). A key profile cannot hold that password: it is asked
  every time. A refused password or agent is reported, not turned into
  a question. A lone hidden "password" question gets the stored password
  when the password method was not offered. Questions reach the owner as
  `SessionEvent::Prompt(LoginPrompt)` (while `Connecting`); answer with
  `SessionManager::answer(id, answers)`, refuse by closing.
- `src/known_hosts.rs` — trusted SSH host keys, per host and port
  (names ignoring case). `KnownHosts::in_data_folder(dir)` = the file
  `<data folder>/known_hosts` in OpenSSH format (`host` for port 22,
  `[host]:port` otherwise), read again on every check; `in_memory()` for
  tests. `check` → `HostKeyStatus::Trusted | Unknown | Changed { stored }`
  (any stored key that differs counts, even of another type); an
  unreadable file or a damaged entry for the host is an error, never
  `Unknown`. `remember` replaces the host's keys, keeping every other line.
  `SessionManager::new(known_hosts)` / `with_output_options(known_hosts,
options)` take the store. In `ssh.rs` the russh `Client` handler checks
  the key; an untrusted one reaches the owner as
  `SessionEvent::HostKey(HostKeyQuestion { host, port, key, stored })`
  (`stored` empty = first use, else `changed()`), decided with
  `SessionManager::decide_host_key(id, HostKeyDecision::TrustAndRemember |
TrustOnce | Cancel)` (or closing). Cancel → `OpenError::HostKeyNotTrusted`
  / `HostKeyChanged`; store failures → `OpenError::KnownHosts`. The
  connect timeout is paused while the user decides and restarts after. Host certificates are never offered, so one is refused. Known
  limit: writes are serialised per process only.
- `src/session/algorithms.rs` — what SSH sessions offer and the
  legacy-crypto switch (`docs/legacy-crypto.md` is the user doc).
  `SshTarget.legacy_crypto` / `JumpHost.legacy_crypto` (each hop its
  own; `ResolvedHost::ssh_target_with` copies the effective setting, quick
  connect is always off) pick `algorithms::preferred(bool)`: our own modern
  lists (russh's defaults plus ECDH-nistp kex and aes128-gcm, minus
  `ssh-rsa`), then `LEGACY_ALGORITHMS` appended. `offered_algorithms(bool)`
  gives the lists; `UNSUPPORTED_ALGORITHMS` documents what russh lacks.
  russh's `NoCommonAlgo` only has the one failing list, so `ssh.rs`
  connects through `recording.rs` (keeps the first 64 KiB the server
  sends, dropped once connected) and `parse_offer` reads the server's
  KEXINIT: `OpenError::Negotiation(NegotiationFailure { kind, offered,
legacy_crypto })` with `legacy_crypto_would_help()` /
  `unsupported_even_with_legacy_crypto()` (simulates negotiation);
  `CloseReason::negotiation_failure()` also unwraps a jump host's.
  Negotiated algorithms come from the `kex_done` handler callback and are
  sent once as `SessionEvent::Algorithms(Vec<NegotiatedAlgorithms>)` (jump
  hosts first) just before `Connected`; `.legacy` lists the outdated ones.
  The shell forwards it on `session_open`'s `onAlgorithms` channel and
  puts a `negotiation` object in the closed state; `TerminalSession` shows
  the `[data-legacy-crypto]` header marker and `NegotiationFailure.svelte`
  (`[data-negotiation-failure=legacy-would-help|unsupported|no-advice]`).
- Connection failures and reconnecting (#15). `SshTarget.connect_timeout`
  (`ResolvedHost::ssh_target_with` copies the `connect_timeout` setting;
  quick connect uses the default) bounds each wait on the network while
  opening, per hop: TCP (`tcp::connect` gives each resolved address its
  share), handshake, every login step, the shell start (`within` /
  `deadline` in `ssh.rs`). Time spent waiting for the user (host key,
  login questions) does not count: `UserWait`, shared by `Prompter` and
  `HostKeyAsker`, flags it and the clock restarts after each answer.
  Open sessions send SSH keepalives every connect timeout; 2 unanswered
  (`KEEPALIVES_UNANSWERED`, ~3 timeouts of silence) → `ConnectionLost`.
  Failure kinds: `OpenError::NameNotResolved | Unreachable |
ConnectionRefused | TimedOut { after, stage } | AuthenticationFailed`
  (`stage`: `ConnectStage`, #140, below),
  `ClosedBeforeLogin` (#101: not a byte of SSH before the device hung up,
  as IOS does with every vty line taken), `Connect(String)` for the rest;
  `CloseReason::ConnectionLost`. A logged-in connection with no terminal
  (`FileConnection`, `session/connection.rs`) is in `files.md`. The
  manager keeps a closed session's target until `discard(id)`:
  `reconnect(id, size)` → new `SessionEvents`, same id (`StillOpen` if
  open), to the same target (quick connections; a session to a host
  reconnects as worked out again, below); `duplicate(id, size)` → a new
  session with the same target, login and origin. Shell:
  `session_reconnect` / `session_duplicate` (new channels
  each time; a Tauri channel cannot be reused) / `session_discard` (the
  interface discards on unmount instead of closing). Interface:
  `openSession(source, size, handlers)` (`source` = target or
  `{ kind: "duplicate", of: id }`); `Session.reconnect(size)`. A closed
  `TerminalSession` keeps its scrollback, shows `[data-session-ended]`
  ("Press Enter to reconnect") and Enter in the terminal reconnects,
  writing a `--- reconnecting ---` marker; once `session_reconnect`
  answers, the terminal's size is sent again if it changed meanwhile (the
  ended bar goes at once, and the resize it causes could reach the old
  connection: the device stayed two rows short, found by the smoke on a
  busy machine while doing #121). Duplicate opens a new tab
  beside the original; "New connection" on a closed session dismisses
  its tab (connect view in that pane).
- What to check next (#116). `OpenError::advice()` / `CloseReason::advice()`
  → `Option<String>`, a sentence for the user: `NameNotResolved`,
  `Unreachable`, `ConnectionRefused { port }` (the Port setting, SSH or
  telnet on), `TimedOut` (by stage since #140: `ConnectStage::advice`),
  `AuthenticationFailed` (username and password; a profile's are changed
  in the Credentials view, then connect again), the three agent errors,
  `HostKeyChanged`; `JumpHost` wraps its error's advice as "For the jump
  host X: …"; everything else (and `ConnectionLost`, `ClosedLocally`)
  `None`. The shell's `StateMessage::Closed.advice`; the interface shows
  it (see `window.md`), then "Press Enter to reconnect." (since #130).
  Tests: `connection_errors.rs`
  (`a_failure_says_what_to_check_next`, `a_refused_port_comes_with_its_advice`).
- Which stage did not answer (#140). `OpenError::TimedOut { after,
  stage: ConnectStage }`; the text is `"{stage} within {after} s"`, made
  in the core only (the shell's closed state, jobs and the Files tab
  carry `to_string()`; the interface shows it; the cli's tunnels get the
  core's text through `tunnel.rs`). Stages and words:
  `Tcp { host, port, via: None }` "no TCP connection to H port P",
  with `via: Some(jump)` "... through the jump host J" (the
  `direct-tcpip` request the last jump host never answered, and
  `JumpConnections::forward` for tunnels, untested there); `Identification` "connected,
  but no SSH identification line came"; `KeyExchange` "the SSH
  identification line came, but the key exchange did not finish";
  `Login` "the key exchange finished, but the login got no answer";
  `Channel` "logged in, but the request for a session channel got no
  answer"; `Shell` "logged in, but the shell did not start";
  `Subsystem { name }` "logged in, but the request for the sftp
  subsystem got no answer". A jump host's is wrapped as before:
  "jump host bastion: the key exchange finished, but ...". Each has its
  own `advice()` (Tcp: address, device up, firewall, and with `via` that
  the jump host reaches it; Identification: the Port setting, not an SSH
  server; KeyExchange: a slow device; Login: RADIUS/TACACS+; Channel,
  Shell, Subsystem: busy or out of sessions; all end with the Connect
  timeout). How (`ssh.rs`): `within(timeout, user, &Progress, work)`;
  `Progress` (an `Arc<Mutex<Step>>`) is set by the work as it goes
  (`connect`: Tcp, then `Step::Handshake(FirstBytes)` once the socket or
  the jump host's channel is there, then Login once `connect_stream`
  returns; `open_shell`: Channel, then Shell after the channel opened;
  `Connected::subsystem`: Channel then Subsystem; `exec`: Channel, the
  command's request waits for nothing) and read when the deadline fires.
  Identification versus key exchange is read from what the device sent
  (`FirstBytes::has_identification`: a line starting `SSH-` ended by LF;
  lines before it do not count), since russh calls no handler between
  the two. `tcp::connect`'s own timeout (telnet's only one) gives `Tcp`.
  Unchanged: the limits and when each starts (one deadline per hop for
  connect-to-login, one for the shell start, the clock restarting after
  each answer of the user), `blame_jump_hosts` (still matches any
  `TimedOut`), keepalives. Telnet has only the TCP stage: a telnet
  session is `Connected` once TCP is, and no timeout waits for a prompt
  (a job's "no prompt after logging in" is the job's own, over SSH).
  Before the TCP connection, the agent listing (`agent::open`, its own
  limit) and reading the key run inside the hop's deadline too; a wait
  there would read as the TCP stage (not met, not tested). Tests:
  `tests/connect_stages.rs` (one per stage, lines before the `SSH-` line,
  the first and the second jump host named with their stage, a device
  the jump host gets no TCP connection to, telnet, a file connection's
  login, SFTP's channel and subsystem requests); `edge_jobs.rs`
  `hosts_failing_in_every_way_at_once_each_end_with_their_own_reason`
  (a job's three timeouts now read differently); `edge_ssh.rs`,
  `edge_jump_hosts.rs`, `connection_errors.rs` compare the stage.
  Support: `UnansweringPort` (a listener that never accepts, its backlog
  filled: Linux drops further SYNs; Windows refuses them only after
  about 2 s, so the tests use 1 s), `Stage::Subsystem`, and the test
  server's `direct-tcpip` now connects on the side (a jump host keeps
  answering pings while its connection to the device hangs, as sshd).
  Left as they are: the SFTP and SCP stall timeouts (`FileError::TimedOut`,
  `ScpError::TimedOut`, "the device did not answer within N s"): each is
  one request of a transfer or a listing, so the stage is already known
  from what was being done. An `sftp` subsystem accepted whose first
  SFTP request is never answered is found as "no SFTP" (not a byte of
  SFTP from the device: `NoSftp::ClosedBeforeReply`, which leads to the
  SCP fall back), not as a timeout: tried while doing #140, unchanged
  and untested. Not verified on a real device.
- Reconnecting a host with what it has now (#130, `session/origin.rs`;
  findings 11 and 13 of #116). A session to a workspace host is opened
  from `Profiles::host_session(id, HostWay::Ssh { typed } | Telnet |
Serial)` → `HostSession { target, origin: HostOrigin, login:
SessionLogin, note }` (through `profile_target` / `prompted_target` /
  `telnet_target` / `serial_target`) and `SessionManager::open_host`,
  which keeps the `HostOrigin` in the entry (workspace root, host id, the
  host's name and protocol setting then, and the login: the profile's
  name, or the `TypedLogin` with the usable profile it was typed beside;
  `Debug` hides the password; dropped with `discard`). `origin(id)` gives
  it (`None` for `open` / `open_with`: quick connections).
  `Reconnection::work_out(&origin, Option<&Profiles>)` decides, and
  `reconnect_host(id, HostSession, size, OpenOptions)` reconnects in
  place with the new target, options and origin. The rule (module doc):
  a workspace not open (or none) → `AsMade { note }` (the shell calls
  `reconnect`); the host gone (deleted, moved outside, its file a
  problem) → `ReconnectRefused::Gone`; its protocol setting changed since
  the last connection to one that is not the session's transport (SSH
  opened on purpose on a telnet host stays SSH) → `ProtocolChanged`; no
  address (not serial) → `NoAddress`; a broken jump chain, no serial
  port, an unreadable store → `Unusable`
  (each has `advice()`). Then: profile login → the host's usable profile
  now (`note` "Logging in with the credential profile X, which r1 has
  now." when its name differs), else `SshAuth::Prompt` with an empty
  username (the device's "Username: "/"Password: " questions, as a jump
  host without a profile asks) and a note saying why (no profile now, or
  X not set up: "...or set the profile up in the Credentials view"); the
  origin keeps the old profile name, so it is used once back. Typed login
  → kept with the host's settings of now, unless the host has a usable
  profile now other than the one it was typed beside (none, or a
  different name, or set up meanwhile): then the profile, with the note,
  and the typed login is dropped. The password fall back (typed beside
  the agent's profile) is thus kept while the host keeps that profile.
  Telnet/serial → the host's target of now. A host renamed or moved in
  the app keeps its sessions (#99): `SessionManager::move_hosts(root,
  moves)` (called by `history::carry`) points their origins at the new
  ids. Shell: `session_open` builds `HostWay` from `ConnectTarget`;
  `host_options` gives the log (`host_log`, the `logging` setting now)
  and, over SSH, `LoginBackup::plan` with the `HostSession.login` — both
  worked out again on a reconnect, so a host's logging and
  backup-on-login settings and profile of now apply (the backup's
  "plan made at open" limit of #46 is gone). `session_reconnect` →
  `Reconnected { consumer, note }`, or `ReconnectFailure { reason,
  advice }` (nothing tried). Interface: `Session.reconnect` resolves to
  the note, written dim under `--- reconnecting ---`; a refusal is the
  closed state's reason and advice (`reconnectFailure` in `session.ts`).
  Every window's terminal goes through the same command. No automatic
  reconnect exists for terminal sessions (only Enter or the menu's
  Reconnect); the terminal size is still sent once the reconnect answers.
  Known limits: the session's tab label, header title and jump host
  marker and `SessionRecord` stay what it opened with (as for a moved
  host); a reconnect that switched a typed login to a profile does not
  offer the agent card's password login (the tab's `profileLogin` is
  from the open); Duplicate still copies the original's target (it
  carries the origin, so the copy's own reconnects work it out again);
  the note can in theory land after the first output (it is written when
  the command answers). Tests: `tests/host_reconnect.rs` (corrected
  profile password, corrected port, jump host set meanwhile, host
  deleted, protocol switched to serial, SSH on a telnet host, typed login
  kept with new settings and in a duplicate's origin, typed beside its
  profile kept, typed giving way to a new profile and to one set up
  meanwhile, profile gone or not set up asking, profile renamed on the
  host, a host renamed in the app, quick connection, workspace not open,
  size of now). Smoke: `credentialProfiles` (wrong password in the
  profile, Enter: refused with the advice and "Press Enter to
  reconnect."; corrected, Enter: connected).
- Telnet (#16, `session/telnet.rs`). `SessionTarget::Telnet(TelnetTarget
{ host, port, connect_timeout })`; no login in the protocol: the device
  asks in its output and the user types in the terminal. On connect it
  offers `WILL NAWS`, `WILL TTYPE`, `DO SGA` (an `Agreement::Asked`
  state, so the device's answer is not answered again: no loops), and
  takes the device's requests: NAWS (size sent once agreed and on every
  resize), TTYPE (`TERM` = `xterm-256color`, shared with SSH), ECHO
  (device echoes if it offers `WILL ECHO`; otherwise the session echoes
  typed input itself: printable bytes and tab, CR as CR LF, BS/DEL as
  `\b \b`, never control characters or escape sequences), SGA both
  ways; the rest refused.
  `0xFF` doubled both ways; Enter (CR) goes out as CR LF, a LF right after
  a CR is dropped; incoming CR NUL → CR. A writer task sends input so
  reading continues meanwhile; reads only inside `room_to_read()`. Device
  closing → `RemoteExited { exit_status: None }`; read/write error →
  `ConnectionLost`. A link that goes silent is noticed by a timing-mark
  probe and TCP keepalive (#133, "Links that go silent" below).
  `SessionTarget::encrypted()` / `SessionManager::encrypted(id)` (false
  for telnet). `Profiles::login(id)` follows the resolved protocol and
  returns `HostLogin::Telnet` for telnet (`HostLogin::Serial` for
  serial); `login_with(id, Protocol)` picks one;
  `profile_target` is always SSH. `Profiles::telnet_target(id)` works for
  any host (its port or 23) and fills `TelnetTarget.jump_hosts` as
  `Profiles::jump_hosts` does (#26 item 7; `ProfileError::
  TelnetThroughJumpHost` is gone). `telnet::run` then opens the chain with
  `ssh::open_jump_chain` (host keys and logins asked as for SSH, the
  questions named "Jump host <id>"), takes a `direct-tcpip` channel from
  the last hop (`JumpConnections::forward`, the telnet connect timeout)
  and runs telnet over `Channel::into_stream()` split in two (the socket
  path's `set_nodelay`/keep-alive do not apply; the timing-mark probe
  does). The jump connections drop when the session ends. Login banners
  of the jump hosts are not shown. `SessionRecord::for_host` gives telnet
  its jump host (header "via X"; the Unencrypted marker's title names the
  leg from it). Jobs (`SkipReason::TelnetBehindJumpHost`) and script
  tunnels still skip such hosts. Tests: `jump_hosts.rs`
  `a_telnet_host_with_a_jump_host_connects_through_it` and the three
  after it; `telnet_session.rs`
  `a_telnet_host_behind_a_jump_host_goes_through_it`.
  Credential profiles are not used for telnet logins (nothing types
  them). Tests: `tests/telnet_session.rs` against `support/telnet.rs`
  (`TestTelnetServer::start()` / `start_with(EchoMode::Remote|Line)` /
  `start_waiting_for_offers()` (never sends `DO NAWS`/`DO TTYPE`),
  scripted device: `Username: `/`Password: `, then `size`, `term`,
  `echo off|on`, `binary`, `flood`, `exit`; unknown lines print
  `UNKNOWN_COMMAND` + the line). Shell: `ConnectTarget::Telnet { id }`,
  `host_login(id, protocol?)` → `LoginView::Telnet`, `session_open` /
  `session_duplicate` return `{ id, encrypted }` (`Session.encrypted` in
  the interface). Interface: `prompt(host, protocol?)` in `App.svelte`;
  the side panel has `Connect` (`[data-connect=default]`) plus
  `[data-connect=ssh|telnet]` for the ways that are not the host's own
  (Telnet is offered behind a jump host too since #26; `otherWays(effective)`
  in `ways.ts` blocks only serial without a port);
  `TerminalSession` shows `[data-unencrypted]` ("Unencrypted").
- Serial (#17, `session/serial.rs`). `SessionTarget::Serial(SerialTarget
{ port, parameters: SerialParameters })` (camelCase serde; default 9600
  8N1 no flow control). The system seam is the `SerialPorts` trait
  (`list`, `open` → `Box<dyn SerialLine>`: `read`/`write` wait at most
  `SERIAL_POLL` (100 ms) and return `Ok(0)` for nothing, `set_break`,
  `check`, `try_clone`); `SystemSerialPorts` = the `serialport` crate
  (blocking; DTR on at open, RTS on unless hardware flow control, both
  best effort: a pseudo terminal has no modem lines and refuses them
  with ENOTTY, which made a console forwarded with `socat pty,...`
  unopenable until #58; mark/space parity refused; descriptions drop a
  trailing `(COMn)`). `tests/serial_pty.rs` (Unix) drives the real
  driver on a pseudo terminal pair (#58).
  `SessionManager::new`/`with_output_options` use the system's;
  `with_serial_ports(known_hosts, options, Arc<dyn SerialPorts>)` injects
  another. `serial_ports()` lists, naturally sorted (COM3 < COM10).
  Per port: a reader thread `serial reader <port>` (waits for
  `room_to_read` via a park-based `wait_on_thread`, no runtime needed;
  counts `flow.read` itself, then `Link::forward_counted`; while paused it
  calls `check()` every poll so an unplug is noticed) and a writer thread
  (input and breaks in order; `BREAK_DURATION` 500 ms). `send_break(id)`
  (`SessionError::BreakUnsupported` for other transports);
  `transport(id)` / `SessionTarget::transport()` → `Transport`. A
  failing read/write/check → `CloseReason::PortLost { port, reason }`
  ("…stopped working; was the adapter unplugged?"); open failures →
  `OpenError::SerialPort { port, reason }`. `Closed` is sent only after
  both threads let go of the port (so reconnect works at once).
  `encrypted()` is true for serial (no network). Profiles:
  `serial_target(id)` (any protocol; `ProfileError::NoSerialPort`).
  Shell: `ConnectTarget::Serial { id }` / `QuickSerial { port,
parameters }`, `serial_ports`, `session_break`, `Opened.transport`.
  Interface: QuickConnect has `[data-quick-mode=ssh|serial]`;
  `SerialPortList.svelte` (`[data-serial-ports]`, `[data-serial-port=…]`,
  `[data-serial-refresh]`); SettingsPanel's `serial.port` row has a
  datalist of detected ports; TerminalSession's menu offers
  `[data-send-break]` for connected serial sessions. Tests: `tests/serial_session.rs` over
  `support/serial.rs` (`VirtualSerialPorts`: loopback-TCP-backed virtual
  pair; exclusive open, records parameters, breaks, reader thread names;
  `VirtualDevice::unplug`). No com0com here: the `serialport` driver
  calls are only exercised by `system_*` tests (list; open a missing
  COM250) and the smoke's `serial` part. Break, unplug, DTR/RTS on a real
  adapter: not verified yet. Never open the machine's real ports in tests
  (this one's COM4 is not a console).
- Session logging (#20, `src/logging.rs`). `LogFolder::new(root)`:
  `host_folder(&ws, id)` = `<root>/workspaces/<workspace folder name>/<tree
path>/<host file stem>`, `host_log(&ws, id)` → `Option<SessionLog>`
  (`None` when the resolved `logging` is off), `quick_log(name)` =
  `<root>/quick connect/<portable(name)>`, `create_host_folder`.
  `SessionManager::open_logged(target, size, Option<SessionLog>)` (`open`
  = unlogged); the log is kept in the entry, so reconnect/duplicate log
  too, each connection its own file `YYYY-MM-DD_HH-MM-SS[_n].log` (local
  time, chrono), created on `Connected` (or earlier output). `Link`
  tees output to a `Logger` (sync `write_all` per chunk on the session
  task; complete lines only, the unfinished last line on close) and
  finishes it before sending `Closed`. Any failure →
  `SessionEvent::LogFailed(LogFailure { path, reason })`, once; logging
  stops, the session goes on (`next_state` panics on it). Text:
  `PlainText` (public; push → complete lines, finish → the rest) drops
  sequences and applies BS/CR/CSI K,X,P,@,C,D,G within a line (lines
  capped at 4096 columns), trims trailing blanks, LF endings. Shell:
  log root = `app_log_dir()` (local app data `logs`), or
  `<ENODIX_DATA_DIR>/logs`; `logs_open_folder(id)` (tauri-plugin-opener
  `open_path`); `on_log_failure` channel on open/reconnect/duplicate.
  Interface: `[data-open-logs]` in HostDetails, `[data-setting-note=
logging]` (secrets warning) in SettingsPanel, `[data-log-failed]` bar in
  TerminalSession. Known limits: two workspaces with the same folder
  name share a log folder; a stalled disk blocks the session task;
  nothing checks the log root is outside the workspace; the smoke does
  not click "Open log folder" (it would open Explorer). Tests:
  `tests/session_logging.rs` (temp folders only).
  A host renamed or moved while a session to it is open (#99): the
  session carries on in the new place. `SessionLog` shares its folder
  between clones (`folder()` returns a `PathBuf` now), so
  `SessionManager::move_logs(from, to)` (called by `history::carry`
  before it moves the files) points every entry logging to `from`,
  closed ones and a connection still connecting included, at `to`; the
  open log file is renamed while open (Rust opens files with
  `FILE_SHARE_DELETE` on Windows) and the `Logger` writes on into it,
  naming it by the session's folder now in a failure. The next
  connection logs to `to`. When the new place holds an earlier host's
  logs nothing moves and the sessions keep the old folder. Tested in
  `tests/host_moves.rs` on Windows (and the Linux checks).
- Handing a session to another consumer (#82, `src/session/relay.rs`):
  `Relay::start(events, consumer)` owns a session's `SessionEvents` and
  delivers them to one `Consumer` (`deliver(event) -> Result<(),
  ConsumerGone>`, `gone(self: Box<Self>)`) at a time. `release(key)` stops
  delivery to that consumer (nothing more reaches it once it returns) and
  returns the output delivered in all (bytes): the end of its share.
  `attach(consumer)` → `Attached { key, offset }` (refused with
  `HandoverError::Shown` while another is attached; events that came in
  between wait for it, in order). Acknowledge with `acknowledge(key,
  bytes)`: a released consumer's unacknowledged output counts as
  acknowledged, and its late acknowledgements are ignored. A failing
  `deliver` lets go of the consumer the same way (the event waits for the
  next one; the session stays open). `deliver` runs with the relay
  locked: it must not block or call back into the relay. Dropping every
  `Relay` handle drops the events at once, attached or not, which closes
  the session as dropping `SessionEvents` does (at once, paused or not,
  since #118). The relay learns that a consumer died only from a failed
  `deliver`, and a Tauri `Channel::send` into a crashed or closed WebView2
  page still returns `Ok` (seen in the #82 prototype), so the shell
  notices a dead session window's page itself (#89: WebView2's
  `ProcessFailed`, WebKitGTK's `web-process-terminated`; `window.md`) and
  lets go of its sessions (`layout::take_over_dead_window`). A hung one
  (#120) is found by the shell's own questions (`layout::PageAnswers`)
  and its sessions taken the same way once the user asks; the engine
  ending as a whole ends the app (`window.md`). What
  remains: a dead main window's page is not noticed (its sessions stay
  held by it until the app is closed), and on Linux the watch is built
  and compiled but not yet run on a desktop (#58). Consumer
  keys come from one process-wide count, so no two relays give out the
  same key (a reconnected session's new relay ignores the old relay's).
  Tests: `tests/session_handover.rs` (SSH flood, telnet flood, serial,
  logging in one file, host-key question while held, a reconnect's new
  relay and the old key). The app prototype is on the branch
  `prototype/82-session-handover`, its findings on #82.
- Every session of the app goes through a relay (#83,
  `src/session/relays.rs`). `SessionRelays` keeps each session's `Relay`
  and the `WindowId` it is shown in (`shown_in(session)`).
  `start(session, window, events, consumer)` → the consumer key; it
  replaces the session's relay (a reconnect's new events get a new
  relay). `acknowledge(session, key, bytes)` passes on to
  `Relay::acknowledge` (a wrong or stale key, or an unknown session, is
  ignored, never an error). `forget(session)` drops the relay, which
  closes the session. A consumer found gone on delivery has its relay
  dropped (only if it is still the session's: relays are numbered),
  closing the session as dropping `SessionEvents` did before: the main
  window's sessions go with its page. A session window's found gone
  leaves its session open and held by nobody, for the main window (#84).
  Moving (#84): `release(session, key)` (→ the end of its share),
  `attach(session, window, consumer)` (→ `Attached`; the window it is
  shown in changes; refused while held, `HandoverError::NoSuchSession`
  without a relay), `let_go(session)` (whoever holds it, for a window
  that died holding it; `Relay::let_go`), `reported(session)` (since #86,
  replacing `state`: `Reported`, the last state, algorithms, log failure
  and backup outcome delivered to any consumer of the current relay,
  `Relay::reported`: where a session taken over stands before the events
  that waited), `live(session)` (#88: a relay whose last reported state
  is not closed, nothing reported counting as opening; what the closing
  questions count). Tests:
  `tests/session_relays.rs` (wrong and stale keys ignored and right ones
  make room, gone closes the session, reconnect, forget; release and
  attach in another window, a wrong key refused, a dead window let go
  of, a session window found gone leaving its session open, the last
  state kept, live while opening and connected; #89: a dead window's
  flooding session handed to main and flowing again, the main window or
  one gone refused). The app's record of each session is
  `session::SessionRecord` (`terminal.md`, paste safety; tests
  `tests/session_record.rs`).
  Shell: a managed `SessionRelays`; `Channels` (the window's channels)
  is the `Consumer` (each event to its channel, a backup outcome also
  emits `config-saved`; a failed send is `ConsumerGone`).
  `session_open`, `session_duplicate` and `session_reconnect` start the
  relay and return the key (`Opened.consumer`, `session_reconnect`'s
  result); `session_acknowledge { id, consumer, bytes }`;
  `session_discard` forgets the relay. `SessionManager::acknowledge` is
  no longer used by the shell. Interface: `session.ts`' `channels()`
  holds the id and key from `ready(id, consumer)` and acknowledges with
  both. The smoke test covers output, prompts, host keys, reconnect and
  backups through the relays.
- The russh `Client` handler (`ssh.rs`) has a second job for file
  connections only (`Client.followed`, `None` for terminal sessions and
  jump hosts): it tells why the device's connection ended (`Followed.ended`,
  #101) and notes each channel the device ends in order (`exit_status`,
  `channel_eof`, `channel_close` into `Followed.channels_ended`, #108).
  Who uses them and why is in `files.md`.
- Edge testing (#118): `tests/edge_ssh.rs`, `edge_telnet.rs`,
  `edge_serial.rs`, `edge_jump_hosts.rs`, `session_resources.rs` (memory
  and leaks, a file of its own). What was tried and holds: floods read
  fast (32 MiB; 512 MiB `--ignored`), slowly and not at all (memory grew
  3 MiB through a 64 MiB flood), closed mid-flood; a link that stalls with
  no FIN (`support::proxy::LinkProxy`) or is cut mid-command, at each hop
  of a two-hop chain too, during the device's login and during a session
  (lost within about three connect timeouts: 1.5 s at 500 ms, 3 s at 1 s); a device
  closing or going silent at each stage (before the banner, after its
  version line, at login, channel open, shell request, mid-output); a
  device ending its shell with output in flight (all of it arrives
  first); sleep and wake (the device timing the session out while
  nothing crossed: `ConnectionLost`, then reconnect); twenty sessions
  flooding at once; fifty opened and closed in a row (tasks, handles,
  device connections and log files all back where they were); rekeying
  every 64 KiB and 500 ms (dozens of key exchanges, output whole); 15
  keepalive rounds on an idle session; a 57 KiB login banner full of
  control sequences; a megabyte before the shell is confirmed; text that
  is not UTF-8 and characters split across reads (delivered byte for
  byte; the log keeps them whole); a console a byte at a time; a login
  answered after seconds; a three-question keyboard-interactive login;
  megabyte pastes (through `plan_paste`, line by line) into slow telnet
  and serial devices, 100 KiB into an SSH device echoing a byte per
  message (1 MiB `--ignored`); closing while a paste waits.
  Faults found and fixed: dropping a paused session's events (a window
  whose page died while its output was held) left the session, its task
  and its connection open for ever: `Link::command` now also ends when the
  events' reader is gone. A device hanging up during the login was
  reported as "authentication failed" (russh answers a closed connection
  as a refusal listing no methods; a real refusal lists them, and a
  device refusing then hanging up is still "authentication failed"):
  `authenticate` and `connect_and_log_in` check the connection and say
  "the device closed the connection during the login" (or "the
  connection broke during the login (...)" from the reason russh gave:
  `Client.ended`); the channel open on a closed connection says it closed
  before starting a shell. A jump host whose connection ended while the
  session logged in to the device is named (`lost_jump_host`:
  `OpenError::JumpHost`). Telnet: answers to the device's requests are
  counted (`PendingReplies`) and reading waits while 64 KiB of them are
  unsent (a device negotiating without end and never reading made them
  pile up: 177 MB in 2 s); an option the session does not take is
  refused at most 3 times (`MAX_REFUSALS`; a device asking again after
  each refusal kept both answering, 17 000 times a second). Logs:
  `PlainText` keeps at most 256 parameter characters of a control
  sequence (an endless `ESC [ 1;1;…` grew without bound).
  Known limits, left as they are (the first three were taken up by #133,
  below): telnet had no keepalives; while an SSH
  session's output is paused a dead link is not noticed until reading
  resumes (still so); a jump host going silent during
  the device's login read as the device timing out; a login banner over russh's 256 KiB packet limit
  fails the login ("the connection broke during the login (Bad packet
  size ...)"; still so), and the banner was never shown (shown since
  #134, below);
  a device sending output without end before confirming the shell is
  only bounded by the connect timeout (`early_output`); combining and
  wide characters count one column each in the log's line editing.
- The SSH login banner (#134). What a device sends before the login
  (RFC 4252 `SSH_MSG_USERAUTH_BANNER`: `banner login` on IOS,
  `/etc/issue.net` on Linux) reaches russh's client `Handler::auth_banner`
  as a `&str` (decoded as UTF-8 by russh: a banner that is not UTF-8
  fails the login, untested). `OpenOptions.login_banner` (default off)
  makes it the session's output: the shell sets it for every terminal
  (`quick_options`, `host_options`); jobs, config backups, back up on
  login (second channel or second connection: `OpenOptions::default`),
  second channels, file connections (`Prompter.banners` `None` in
  `connection.rs`) and tunnels (`open_jump_chain_unattended`) leave it
  off, so a banner never reaches a job's output, a kept run or a config
  backup. How: `ssh::run` gives its `Prompter` and `HostKeyAsker` one
  channel (`Link::askers_into`) and, with the option, `Prompter.banners`
  (`ssh::Banners`) a sender into the same channel; each hop's `Client`
  handler gets a `HopBanners` (its jump host's name, or `None` for the
  device) and sends the banner there as `SessionEvent::Output`. The run
  loop (biased `select!`, as `FileConnection::open`) passes them on in
  order with `Link::opening`: output through `Link::output` (counted by
  flow control, logged, acknowledged like any output), questions as they
  are; what is left is drained before `Algorithms`/`Connected` or the
  failure. Why output and not an event of its own: the log, flow control,
  the relays, the shell's channels, session windows' snapshots and the
  terminal all handle output already; a new event would need each of
  them (and the job engine's exhaustive matches) to learn it. Russh calls
  the handler before it answers the login request the banner came with,
  so the banner is in the channel before any question that follows.
  Shown text (`HopBanners::show`): empty banners are dropped; each bare
  `\n` becomes `\r\n`, a last line without an end gets `\r\n`; control
  sequences go through untouched (the hop's host key was accepted before
  the banner could come: the key exchange is first; it is still device
  output, and the terminal drops what xterm answers to it while
  connecting, as input then is dropped). A jump host's banner is preceded
  by a dim line `\x1b[2m--- login banner of the jump host <name> ---
  \x1b[0m` (`<name>` = `JumpHost.name`, the host id), and the device's,
  when a jump host's was shown first, by `--- login banner of the device
  ---`, in connect order. Each connection shows them (a reconnect or a
  duplicate again: the device sends them again). A login that fails after
  the banner shows it before the failure; the log file is then created
  for that connection (it holds the banner). The `SshAuth::Prompt` login
  with a username now asks for the password after the `none` request
  (inside `authenticate`), so the banner comes first (russh sends the
  `ssh-userauth` service request only with the first login request, and
  devices send the banner then); with no username (a jump host or host
  without a usable profile), "Username: "/"Password: " are still asked
  together first, since the username cannot change after the first
  request (OpenSSH disconnects): the banner then comes after the answers.
  Keyboard-interactive questions always come after `none`, so after it.
  A device's host-key question comes before its banner (the key exchange
  is first), but after the jump hosts' banners. The interface: see
  `terminal.md`. Tests: `tests/login_banner.rs` (first output with its
  line ends made right, left out without the option, before a password
  and a keyboard-interactive question, on a failed login, again on
  reconnect and duplicate, none on a second channel, in the log, a jump
  host's named and in connect order, before its question and the device's
  host key, after the answers when the username is asked), `jobs.rs`
  `login_banners_are_not_in_a_jobs_output`, `config_backup.rs`
  `a_login_banner_is_not_in_the_backup`, `login_backup.rs`
  `the_terminal_shows_the_login_banner_and_the_backup_does_not_keep_it`
  (second channel and second connection), `file_connection.rs`
  `a_login_banner_reaches_no_one_and_does_not_stop_the_login`. The #118
  tests in `edge_ssh.rs` (57 KiB banner, megabyte banner) open without the
  option and are unchanged. Smoke: `agentAndKeyboardInteractive` quick-
  connects to the example server's `--banner` 22727. On real devices
  (2026-11-21, lab check `lab_login_banner`, `testing.md`): IOS-XE 17.03
  (CSR 1000v) sends `banner login` as the SSH authentication banner (with
  `\r\n` line ends and a leading blank line) and shows `banner motd` in the
  shell before the first prompt, so the motd reaches jobs as ordinary
  output (a motd line ending in `CHG#` or `<...net>` did not disturb the
  first-prompt rule); cEOS 4.36 writes `banner login` to `/etc/issue`
  (sshd's `Banner`), sent as the SSH authentication banner too, and on
  telnet prints it before `Username: ` with `\r\0\r\n` line ends. Both
  shown with the option, neither without; IOS and EOS keep the text in
  `show running-config` (`banner login ^C ... ^C`, `banner login ... EOF`),
  so a config backup holds it once, as configuration. Not verified: a
  banner that is not UTF-8, several banners from one hop, telnet to IOS
  (the lab's csr1 has no telnet reachable).

- Links that go silent (#133). Time to notice a dead link, T being the
  connect timeout (default 15 s):

  | Transport, case | Windows | Linux |
  | --- | --- | --- |
  | SSH, each hop (keepalive every T, 2 unanswered) | 3T (3.0 s at 1 s) | 3T |
  | Telnet, device answering timing marks | 3T (3.0 s at 1 s) | 3T |
  | Telnet, device never answering / never negotiating: TCP keepalive (T idle, then T apart, 2 probes) | 3T (Windows 10 1703+; else 11T) | 3T (whole seconds, at least 1 s) |
  | the same, with typed input unacknowledged | the system's retransmissions (5 by default; not measured) | 3T (`TCP_USER_TIMEOUT`) |
  | Any session whose output is paused | once reading resumes, then as above | same |
  | Jump host silent while the next hop logs in | T + up to T (ping) | same |

  Telnet (`telnet.rs`): after T of silence from the device while reading
  may go on, `Probes::quiet` asks for a timing mark (`IAC DO TIMING-MARK`,
  RFC 860; `WILL` or `WONT TIMING-MARK` back is an answer, `Received.marks`,
  never refused nor shown). A device that has answered once and then sends
  nothing for `PROBES_UNANSWERED` (2) more probes is `ConnectionLost`, as
  SSH. One that never answered is asked `PROBES_BEFORE_ANSWER` (3) times,
  then left alone: never taken for dead. One that never sent a
  negotiation or subnegotiation (`Telnet.speaks_telnet`: a console
  server's raw port) is never probed, as the bytes would reach the console.
  While reading waits (flow control, or 64 KiB of answers unsent:
  `Flow::has_room`, `PendingReplies::has_room`) silence does not count:
  the answer would wait behind the held output. Why a probe and not TCP
  keepalive alone: a TCP keepalive is answered by the far end's TCP, so it
  only notices a link that drops packets, and the link proxy of the tests
  (like a NAT box or a TCP proxy in the path) keeps acknowledging: it
  cannot be tested here, and does not help through such boxes. The probe
  is answered by the device's telnet server itself, end to end, and the
  proxy can play it. TCP keepalive is still set (`tcp::keep_alive`,
  `socket2` with `all`), for the devices the probe leaves alone: time and
  interval T, 2 probes where the system lets a socket set the count
  (Linux; Windows 10 1703+, `TCP_KEEPCNT`; else Windows' fixed 10), and on
  Linux `TCP_USER_TIMEOUT` 3T for sent data never acknowledged. A healthy
  quiet session costs 3 bytes each way per T (as an SSH keepalive). On the
  lab (the suite's telnet check, `lab_telnet`): a session to cEOS
  4.36.0.1F left quiet through four probes showed nothing and went on
  working; whether cEOS answered the probe or ignored it cannot be seen
  through the public API. Not
  verified: a real device's answer (IOS and BSD telnetd answer `WILL
  TIMING-MARK`; SR Linux and console servers not tried), and TCP
  keepalive actually ending a connection (needs a link that drops
  packets: a packet filter on the lab).
  Tests: `edge_telnet.rs`
  `a_telnet_link_that_goes_silent_ends_the_session_as_lost_in_bounded_time`
  (three probes answered on a healthy quiet link and nothing shown, then
  lost 3 s after the stall), `a_telnet_device_that_never_answers_a_timing_mark_is_not_taken_for_dead`,
  `a_device_that_never_negotiates_is_never_probed`,
  `a_telnet_session_held_back_for_several_timeouts_is_not_taken_for_dead`;
  `session::tcp::tests` (the core's one unit test: the options read back
  from the socket, on Windows with `getsockopt`).

  A paused SSH session (not done, pinned by
  `a_link_that_goes_silent_while_output_is_paused_is_noticed_once_reading_resumes`).
  Why keepalives stop, in russh 0.65.1: the connection's loop
  (`client::Session::run`) decodes a `CHANNEL_DATA` packet and, inside the
  same `select!` arm, `chan.send(ChannelMsg::Data).await`s on the
  channel's bounded queue (`channel_buffer_size`, 100 messages,
  `client/encrypted.rs`). Once the session stops reading the channel
  (flow control) the queue fills and that await holds the whole loop: the
  keepalive timer, outgoing messages and the socket are not polled, so no
  keepalive leaves and none is counted. It is russh's loop that waits, not
  ours: the session's own loop keeps taking commands. Why taking the
  packets anyway (into a buffer of ours) would not do: russh sends the
  window adjust as it decodes (`adjust_window_size` against
  `config.window_size`, whenever the window is below half), not as the
  session consumes, so a device flooding would keep sending and the buffer
  would grow without bound, against the rule that memory stays bounded.
  The `Handler::adjust_window` hook cannot stop it either: the client
  stores its result in `target_window_size` but adjusts to
  `config.window_size` regardless. And even with the loop free, a
  keepalive's answer comes behind the output the device sent first, which
  would have to be read (and held) to see it. What it would take: russh
  letting the application own the channel window (adjust only as output
  is acknowledged), so that the queue never fills and the device is held
  by the SSH window instead; a change to russh (pinned), or our own
  channel handling. TCP keepalive on the SSH socket would not help while
  paused either: russh is not reading the socket to see it fail. So a
  paused SSH session (and a paused telnet one) notices a dead link about
  3T after reading resumes.

  A jump host silent during the opening (`ssh.rs` `blame_jump_hosts`):
  when the wait for a hop (or the shell start) times out, the jump hosts
  before it are pinged in connect order (`Handle::send_ping`, each within
  its own connect timeout; a ping crosses the hops before it, which
  answered) and the first that does not answer is named:
  `OpenError::JumpHost { name, error: Connect("it stopped answering while
  the session was being opened (no answer within N s)") }`; a hop whose
  connection ended is named first, as before (#118). Every jump host
  answering leaves the device's `TimedOut`. Also used in
  `connect_jumps` (a jump host silent while the next one logs in, the
  tunnels' chain too) and for file connections. Tests:
  `edge_jump_hosts.rs`
  `a_hop_going_silent_or_dropping_during_the_login_to_the_device_fails_the_session`
  (core and site named, the router `TimedOut`),
  `a_jump_host_going_silent_while_the_next_one_logs_in_is_named`.

- Security review (2026-11-22): `KnownHosts::remember` refuses a host
  name the file would read as other hosts (a comma, whitespace, a control
  character, `*`, `?`, `!`, a leading `#`, `@` or `|`): trusting a key for
  `core1,x` would have trusted it for `core1`. Test in `host_keys.rs`.
