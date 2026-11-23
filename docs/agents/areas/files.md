# Files

File connections (a login with no terminal) and SFTP on them: what the Files tab (#103) and the transfer engine (#105) stand on. The prototype and its findings are #96 (branch `prototype/96-files`, not merged); the map is #95, closed by its sweep (#108, below). The user's page is `docs/files.md` (what each platform needs, the known limits, the trust facts): keep it in step.

- `src/session/connection.rs` — `FileConnection`, public as
  `enodix_core::files::FileConnection` (#101). It logs in exactly as a
  terminal session does: `ssh::open_connection` and `open_shell` share
  `connect_device`, and the file connection stops before `start_shell` (the jump chain, host-key checks, the login flow, legacy
  crypto, connect timeouts, keepalives all come from `connect_jumps` /
  `connect`). Build the target with `Profiles::profile_target` /
  `prompted_target`, as for a session. Two ways to open:
  - attended: `FileConnection::open(target, known_hosts) -> Opening`
    (returns at once; needs a runtime). `Opening::next()` gives
    `OpeningEvent::HostKey(HostKeyQuestion)` (decide with
    `decide_host_key`), `Prompt(LoginPrompt)` (answer with `answer`; jump
    hosts' questions are named "Jump host <id>" as for sessions), then
    `Opened(FileConnection)` or `Failed(OpenError)`, then `None`.
    Dropping the `Opening` gives up and closes whatever was connected.
    A key trusted once is asked about again on the next connection.
  - unattended: `FileConnection::open_unattended(target, known_hosts)`
    → `Result<FileConnection, ConnectError>`: an untrusted key is
    `ConnectError::HostKeyRefused(question)` (cancelled), a login question
    `QuestionRefused(prompt)` (the answers channel is dropped, so the
    login fails); both display the jobs' reasons
    (`job::host_key_refused`, `job::login_question_refused`, now
    `pub(crate)`). Anything else is `Failed(OpenError)`.
- Ownership: `ssh::Connected` holds the device's connection then the jump
  hosts', nearest first; `close()` (async) or dropping it disconnects them
  in that order (a drop spawns the disconnects on the current runtime;
  with none, dropping the handles ends them in no order). It carries one
  channel at a time: a one-permit `Semaphore`, held by `Sftp` for as long as
  it lives (`StartError::Busy` otherwise). Never a terminal on it. The SSH
  window is russh's default 2 MiB (#96: lower windows cripple downloads).
- Loss: the device's `Client` handler has an `ended` watch (only for file
  connections; `Hop.ended`). russh's `disconnected` callback sets
  `Lost::Closed { message }` (the device sent a disconnect),
  `NotAnswering` (keepalive timeout) or `Broken(reason)`; the handler is
  dropped when the connection ends, which wakes the watch even when
  russh skips the callback. `FileConnection::lost()` resolves with it (or
  `Lost::JumpHost { name }` when a jump host's connection has ended
  meanwhile); `is_open()`. Known limit: when a jump host drops, which
  connection russh notices first decides whether `JumpHost` or the
  device's reason is given (only the device cut is tested).
- `OpenError::ClosedBeforeLogin` (in `session/mod.rs`, for sessions too):
  the device closed the connection before sending one byte of SSH (the
  `FirstBytes` recording is empty and russh says IO or `Disconnect`):
  "the device closed the connection before the login began; all its lines
  may be in use". IOS with every vty line taken does this (#96).
- `src/files.rs` + `src/files/sftp.rs` — SFTP with `russh-sftp` 3.0.1
  (pinned exactly in the workspace `Cargo.toml` beside russh).
  `FileConnection::start_sftp()` asks for the `sftp` subsystem on a new
  session channel (`Connected::subsystem`, within the connect timeout) and
  starts `SftpSession` over `channel.into_stream()`, wrapped in `Answered`
  (notes whether the device sent a byte). Outcomes:
  `Ok(Sftp)`; `StartError::NoSftp(NoSftp::Refused)` (subsystem refused);
  `NoSftp::ClosedBeforeReply` (the channel closed before SFTP's VERSION,
  before or after accepting: IOS-XE, and cEOS below privilege 15);
  `StartError::Connection(reason)` (lost, or no answer); `Busy`.
  **A channel the device closed in order is `NoSftp` even when the
  connection has ended since (#108).** IOS-XE ends the whole connection
  right after closing the SFTP channel it has no server for (as it does
  after an SCP transfer), so by the time `start_sftp` looks, the
  connection is often gone: the lab's CSR 1000v failed a file job with
  "its files cannot be reached: the connection was lost" while its Files
  tab fell back fine (a race). The device's handler (`ssh.rs`,
  `Followed.channels_ended`, a `ChannelsEnded`, only for file connections;
  the handler's `exit_status`, `channel_eof` and `channel_close`) notes
  each channel the device ends in order (an exit status, an EOF or a close
  on it);
  `Connected::channel_ended_in_order(id)` is final once the connection has
  ended (russh runs those callbacks before it marks the connection
  closed, so `start_sftp` reads `is_open()` first, then this). Rules: a
  `Failure` or a close read from the channel before the subsystem's reply
  (`SubsystemRefusal::Refused` / `Closed`) is the device's word whatever
  the connection did since (a channel that ends with only an exit status
  noted counts as closed too); a channel that ended without a word
  (`SubsystemRefusal::Ended`) is `NoSftp` only while the connection is
  up; after the reply, no SFTP byte and (the connection up, or the
  channel ended in order) is `NoSftp(ClosedBeforeReply)`, else a lost
  connection is `StartError::Connection`. A connection that drops without
  the device closing the channel is still not a fall back.
  Settings (private constants in `sftp.rs`, the values #96 measured and
  #105 kept): `REQUESTS_IN_FLIGHT` 64 reads and writes, `WRITE_SIZE`
  32 KiB (about 2 MiB in flight reaches the line; russh-sftp's 16 halves
  uploads), request timeout three connect timeouts (the spec's stall
  rule; russh-sftp's default is 10 s). `Sftp` is `Clone` (one session,
  shared by the browser and the engine).
- `Sftp` operations, paths as the device's strings: `home()` (realpath of
  `.`), `list(folder)` → `Vec<RemoteEntry { name, kind: EntryKind::File |
Folder | Link | Other, size, modified, permissions: "rwxr-xr-x" }>`
  (hidden files included, `.`/`..` dropped, links as links: readdir gives
  lstat attributes), `stat(path)` (follows links; `name` = last part),
  `make_folder`, `rename`, `delete_file`, `delete_folder`, `open_read` →
  `RemoteReader` (`AsyncRead`), `create` → `RemoteWriter` (`AsyncWrite`;
  truncates; `finish()` waits for every write and the close). Streams hold
  the SFTP session (and so the channel) alive; their `io::Error`s carry a
  `FileError` (`get_ref`).
- `FileError` says what happened: `NotFound { path }`, `PermissionDenied
{ path }`, `AlreadyExists { path }`, `NotEmpty { path }`,
  `ConnectionLost`, `TimedOut { after }` (a request's stall; the open and
  the SFTP or SCP channel's start say their stage instead, #140 in
  `sessions.md`), `SessionEnded` (SFTP ended, the
  connection did not), `Refused { action, message }` (the device's message
  only when it says more than its bare code). SFTP v3 answers a bare
  `Failure` for a rename onto an existing name, a folder that exists and a
  folder that is not empty: on `Failure` the core looks (lstat of the new
  name, a listing) and reports `AlreadyExists` / `NotEmpty`. A rename onto
  an existing name is never retried by deleting it (it fails on every lab
  device, #96). An error after the connection ended is `ConnectionLost`
  (russh-sftp wakes pending requests with "sender dropped").
- The Files tab's browsing (#103): `src/files/browser.rs`, public as
  `enodix_core::files::{FileBrowsers, BrowserId, BrowserEvent, Listing,
  BrowseError, parent_folder, join_path, resolve_path}`.
  `FileBrowsers::open(host, target, known_hosts, start, platform)` (since
  #106 the host id the browser belongs to, and its platform for the SCP
  hint) gives `(BrowserId,
  UnboundedReceiver<BrowserEvent>)` (needs a runtime): a task opens the
  connection attended (`HostKey { question }` / `Prompt { prompt }` out;
  `answer`, `decide_host_key`, `cancel` in, through a command channel),
  starts SFTP, lists `start` (falling back to the login's folder with a
  `notice` naming why) and sends `Connected { listing, notice }`, or
  `TransferOnly { reason, hint, folder }` (no SFTP, after `start_files`
  fell back to SCP: #106, below) or `Failed { reason }` ("the login was cancelled"
  for `cancel`); then `Lost { reason }` when `FileConnection::lost`
  resolves. `close(id)` drops the slot at once (`close_all()` since #106 after the cancelled transfers, below) (aborting the task,
  closing the connection). `list(id, folder, path)` (the path box:
  absolute, `flash:`-style, or from `folder`; "" = Refresh), `rename`,
  `delete(id, folder, name, kind)` (folder: `delete_folder`, else
  `delete_file`), `make_folder`; each change returns the folder listed
  again. A listing is the device's realpath of the folder
  (`Sftp::canonical`, crate-private; the given path if that does not
  list), folders first, then by name ignoring case, with `parent` (`None`
  at `/`, `flash:`). New names are refused before the device is asked when
  empty, `.`, `..` or holding `/` (`BrowseError::BadName`); `NotConnected`
  while logging in, lost or closed. `RemoteEntry` and `EntryKind`
  serialise (camelCase kind, `modified` as seconds since 1970).
- The Files tab's transfers (#106), in `src/files/browser.rs` too, public
  as `enodix_core::files::{Added, TransferState, follow,
  existing_locally}`. `FileBrowsers` holds one `Transfers` engine; each
  browser's transfers go under its host id, so they outlive a browser:
  a connected browser gives the engine `Link::sftp` of its session
  (`Transfers::connect`), and after a loss the waiting ones run once a
  new browser for the host connects (a Reconnect). A device without SFTP
  (`Files::Scp` from `start_files`) gives `TransferOnly` and the browser
  keeps only an SCP link (a connection per transfer; no connection held),
  attended since #109: see the next point. Calls: `upload(id, folder, locals)` (each under its
  own name in `folder`), `download(id, folder, names, into)`, `send(id,
  local, remote)` and `fetch(id, remote, into)` (by path: the SCP form;
  the name is the path's last part after `/` or `:`), each giving
  `(Added { id, direction, name, remote, local }, TransferWatch)`; names
  that cannot be files are `BadName` before anything is added. A link the
  engine dropped (an SCP connection that failed) is given again on the
  next add, so a send is never left waiting. `present(id, folder, names)`
  gives the names already on the device (none over SCP);
  `existing_locally(folder, names)`; `cancel_transfer(id)`;
  `close_host(host)` (the tab closed: its browsers and transfers) and
  `close_all` cancel the host's transfers and keep the closed browsers'
  connections until the cancelled one has ended (at most `CLOSE_WAIT`,
  30 s, on the current runtime; at once without one), so a cancelled
  upload removes its partial file. `TransferState` is a `Status` as the
  tab shows it (camelCase `kind`: `waiting`, `connecting`, `running {
  done, total }`, `done { bytes, hashes, size, checked }`, `failed {
  reason, hint, partial }` (the hint is `ScpError::hint`), `cancelled {
  partial }`; the partial and size texts are the core's `Display`s).
  `follow(watch, every, send)` sends the state now, then at most one
  change per `every`, always the last. `TransferId` deserialises.
- Shell (`src-tauri/src/files.rs`, managed `FileTabs { browsers,
  known_hosts }`, the same `KnownHosts` as the `SessionManager`):
  `files_open { host, credentials?, folder?, onEvent }` (main window only;
  `profile_target` or `prompted_target`; `folder` or else
  `Workspace::host_file_folder`; `Workspace::host_platform`; events
  forwarded to the `Channel`),
  `files_answer`, `files_decide_host_key`, `files_cancel`, `files_close`,
  `files_list`, `files_rename`, `files_delete`, `files_make_folder`; since
  #106 `files_upload { id, folder, paths, onUpdate }`, `files_download {
  id, folder, names, into, onUpdate }`, `files_send { id, local, remote,
  onUpdate }`, `files_fetch { id, remote, into, onUpdate }` (each returns
  the `Added` and sends `{ id, state }` on its `Channel` through `follow`,
  every 200 ms at most), `files_present`, `files_existing_locally`,
  `files_cancel_transfer { transfer }`, `files_close_host { host }`
  (async: the core waits on the runtime; `layout_reset` enters the
  runtime for `close_all`).
  `layout_reset` (the main page loaded afresh) and the main window's
  `Destroyed` call `close_all`. `MainRequest::Files` (a session window's
  `main_window_request { kind: files }`) opens the tab in main.
- Interface: `src/lib/files.ts` (IPC, `sizeText`, `timeText`),
  `FilesTab.svelte` (the view `files`, subject the host id; see
  `window.md`).
- SCP (#104): `src/files/scp.rs`, public as `enodix_core::files::{Files,
  Protocol, Scp, ScpError}`. For devices without SFTP; no library speaks
  it, so the core does the least of it (#96 point 5): one file per
  transfer, no folders, no timestamps, on an exec channel
  (`Connected::exec`, opened within the connect timeout; whether the
  device accepted is read from the channel).
  - `FileConnection::scp_send(path, size, source: AsyncRead, cancelled:
    Future)` runs `scp -t <path>`: wait for `\0`, send `C0644 <size>
    <name>` (the path's last part after `/` or `:`), `\0`, the data in
    32 KiB pieces, `\0`, wait for `\0`, EOF, wait up to 2 s (`CLOSE_WAIT`) for the device to close the channel.
    `cancelled` resolving first closes the channel: `ScpError::Cancelled`
    ("a partial file may remain on the device"; SCP cannot remove it; IOS
    removes it itself, cEOS and Junos keep it, #96 point 6). The size the
    device wrote cannot be checked over SCP.
  - `FileConnection::scp_fetch(path, destination: AsyncWrite) -> size`
    runs `scp -f <path>`: `\0`, the header (`T` lines are acknowledged and
    skipped, `D` is refused as a folder), `\0`, exactly the header's size,
    the device's `\0`, ours. The header's name is never compared with the
    path (IOS names `bootflash:ten.bin` as `ten.bin`; OpenSSH's client
    fails on that). Fewer bytes before the channel closes:
    `ScpError::Short { expected, received }`.
  - Reading answers (`Answers`): `\0` is success; `\x01`/`\x02` and a
    line is `ScpError::Refused { message }` in the device's words; any
    other byte starts a text that is the refusal too (cEOS's `% The 'scp'
    command is only available in privileged mode.`, no error byte); a
    channel that closes without an answer gives what was left unread, then
    stderr, then `(exit status N)` (`sh: scp: not found (exit status
    127)`), or `ConnectionLost` when the connection itself ended. An exec
    `Failure` is "it would not run scp". Paths are passed on as given
    (`flash:x`, `/var/tmp/x`). Each wait (a message, a write taking window)
    is bounded by the stall limit, three connect timeouts:
    `ScpError::TimedOut`. Also `Busy` (the one channel is taken),
    `Connection(reason)` (the channel did not open), `Local(reason)` (the
    local file), `Connect(ConnectError)` (from `Scp`'s own connection).
  - Hints, apart from the device's words: `ScpError::hint()` is
    `Some` only for a refusal containing `Administratively disabled`
    ("turn it on with ip scp server enable"); `Scp::hint()` only when the
    fall back happens on a host whose platform is (or answers to) `junos`
    ("for a listing, turn on the SFTP server: set system services ssh
    sftp-server"). Nothing else carries one.
  - The fall back: `FileConnection::start_files(self, platform:
    Option<&PlatformDefinition>) -> Result<Files, StartError>`. SFTP
    starts: `Files::Sftp { connection, sftp }` (the connection stays
    open). `StartError::NoSftp(why)` (refused, or closed before SFTP's
    first reply): the connection is closed and `Files::Scp(Scp)` comes
    back, `Scp::no_sftp()` saying why; `Files::protocol()` tells the
    caller. A failed connection stays `Err(StartError::Connection)`, not a
    fall back. After a fall back the caller gets what SCP answers, never a
    guess: cEOS at privilege 1 looks like no SFTP and then refuses SCP in
    its own words.
  - A connection per transfer: IOS-XE closes the whole connection when a
    transfer ends, and an idle connection holds one of its five vty lines.
    `Scp` keeps the target and known hosts (`FileConnection` now keeps
    what it was opened with, `opened_with()`), and `Scp::send` / `fetch`
    connect unattended, transfer and close; `Scp::connect()` (attended, an
    `Opening`: what the device asks; the Files tab's transfers since #109) and
    `connect_unattended()` let a caller (the engine, #105) do the same
    with its own questions.
  - What SCP cannot do is refused, never worked around with device
    commands: `Files::{list, stat, rename, delete_file, delete_folder,
    make_folder}` on `Files::Scp` give `FileError::TransferOnly { action }`
    ("this device offers transfer only (SCP, no SFTP), so it cannot list a
    folder"); on `Files::Sftp` they are `Sftp`'s.
  - The Files tab's `FileBrowsers` uses `start_files` since #106 and
    shows its SCP form after a fall back.
- The transfer engine (#105): `src/files/transfer.rs`, public as
  `enodix_core::files::{Transfers, Transfer, Direction, TransferId,
  TransferWatch, Status, Outcome, Done, Hashes, SizeCheck, Partial,
  TransferError, Presence, Link, Connect, Connecting}`. The Files tab
  (#106) and the file job (#107) only call it.
  - `Transfers` (`Clone`, one engine): a waiting list per host (the
    caller's host id), one transfer running per host, hosts side by side.
    `connect(host, link)` gives the host its `Link` and starts the next
    waiting transfer (a running one keeps its own); `add(host,
    Transfer::upload(local, remote) | download(remote, local))` →
    `(TransferId, TransferWatch)`; `waiting(host)` in order, `running(host)`,
    `cancel(id)` (a waiting one leaves the list: `Cancelled { Nothing }`; a
    running one stops and deals with its partial file), `close(host)`
    (cancels all, forgets the link), `is_connected(host)`. `add` and
    `connect` spawn tasks: call them within a runtime.
  - `Link`: `Link::sftp(sftp)` (the caller keeps the connection open: the
    browser shares its session), `Link::scp(impl Connect)` (a connection
    per transfer, made by the engine: `Connect::connect()` →
    `Result<FileConnection, ConnectFailure>` (#109: `Failed(reason)`, or
    `Declined(reason)` when the user said no; `From<String>` is
    `Failed`); `Scp` implements it unattended, the Files tab's browser
    gives its own that asks), `Link::from(Files)` (owns
    the SFTP connection; SCP unattended). `presence(path)` (also on
    `Files` and `Sftp`): `Presence::Exists { size }` / `Absent` over SFTP,
    `CannotTell` over SCP.
  - `TransferWatch` is a `watch` receiver: only the latest `Status`
    (`Waiting`, `Connecting` (SCP), `Running { done, total }`,
    `Finished(Outcome)`, the last) is kept, so a watch read slowly or never
    costs the transfer nothing. `status()`, `changed()` (`None` once the
    finished status was seen), `outcome()`. The finished status is sent
    under the engine's lock, so a caller who sees it sees the next transfer
    started (or the host waiting for a link). Speed is the caller's (bytes
    over time).
  - `Outcome::Done(Done { bytes, hashes, size })`, `Failed { error,
    partial }`, `Cancelled { partial }`. `hashes`: an upload's local file,
    MD5 (`md5` crate, already in the tree) and SHA-256, lower-case hex,
    computed as it is read. `SizeCheck::Checked` after an SFTP upload
    (remote stat'ed) and a download (SFTP's stat, SCP's header);
    `NotChecked` for an SCP upload ("the size the device wrote could not
    be checked") and a device that gives no size. A download's check is
    of the local file on disk (its metadata) against the announced size.
  - `Partial`: `Nothing`, `RemovedFromDevice` (a failed or cancelled SFTP
    upload is removed with `delete_file`), `RemainsOnDevice { reason }`
    (the remove failed: Junos read-only, a lost connection),
    `MayRemainOnDevice` (any failed or cancelled SCP upload once
    connected: some devices create the file when the header comes),
    `RemoveUnanswered { after }` (#137: a cancelled SFTP upload whose
    remove went unanswered for `CANCEL_REMOVE_WAIT`, 2 s: "a partial file
    may remain on the device: it did not answer the request to remove it
    within 2 s"),
    `RemovedLocally` (a failed or cancelled download), `RemainsLocally {
    reason }`. An SCP download creates its local file at the first byte
    (`Writing`), so a fetch the device refuses leaves an existing local
    file of that name untouched and reports `Nothing`; an empty file is
    created when the fetch ends.
  - `TransferError`: `Local` ("could not open <path>: ..."), `Device(FileError)`,
    `Scp(ScpError)`, `Lost(Lost)` ("the connection was lost: ..."),
    `Stalled { after }` ("no byte moved for N s: the device stopped
    answering"), `SizeOnDevice { sent, found }`, `SizeDiffers { announced,
    received }`, `Connect` (SCP's connection for the transfer),
    `Declined` (#109: the user said no while it connected; the link
    stays). `Lost` and `Connect` drop the host's link: the waiting transfers wait until the
    caller calls `connect` again or cancels them (a link given meanwhile
    is kept: links are counted). A lost SFTP link is also noticed before
    the next transfer starts.
  - How bytes move: a 256 KiB local buffer (`BUFFER_SIZE`), never the
    whole file; uploads write to the final name (`Sftp::create`
    truncates; no temporary name and rename). The stall rule: each SFTP
    request (create, open, each write and read, `finish`) is bounded by
    `Sftp::stall_limit` (three connect timeouts); SCP's own waits are
    bounded the same way (`ScpError::TimedOut` → `Stalled`). Each SFTP
    step also races `Sftp::ended`: **russh-sftp does not wake a write
    waiting for its answer when the session ends** (a read is woken), so
    without it a cut connection would wait for the stall limit. A
    cancelled SFTP upload drops the writer (russh-sftp sends the close at
    once, in order) and then removes the file, waiting for that at most
    `CANCEL_REMOVE_WAIT` (2 s, #137; it used to wait up to the stall
    limit, 45 s by default, when the device had stopped answering, so a
    Cancel took that long and the app's 5 s close left such a file job
    without an end); a cancel while the size is stat'ed after the last
    write is a cancel too (the file removed the same way). A failed upload
    still waits for its remove as before. Shown by `edge_job_kinds.rs`
    `cancelling_a_file_sent_to_a_device_that_stopped_answering_ends_it_at_once`
    (default 15 s connect timeout; the user's Cancel and `end_all` end in
    about 2 s). The Files tab's cancel goes through the same path. SCP: `scp_send` with the
    cancel future; a fetch is dropped on cancel (`scp_fetch_announcing`
    reports the header's size first, for the progress total); its
    connection is closed after each transfer.
  - Known limit of the stall rule: the engine's own timer bounds each
    256 KiB step, which is close to "no byte moved"; but russh-sftp's
    per-request timeout is also the stall limit (#96 chose it), and a
    request queued behind 2 MiB in flight can wait longer than that on a
    very slow link (about 64 KB/s with a 10 s connect timeout) while bytes
    still move. Not seen on the lab (srl1 at 0.3 MB/s waits about 7 s).
- The file job (#107) sends one file to many hosts through the engine:
  each host gets its own unattended `FileConnection` (`start_files`,
  `Link::sftp` with the job closing the connection after the transfer, or
  `Link::scp`), `presence` for its "already there" rule, and one
  `Transfer::upload`. Its notes are in `jobs.md`. For it the engine's
  `Hashes` deserialise (kept runs), `files::hash_file(path)`
  (crate-private) hashes a local file the way an upload does, and
  `SessionManager::known_hosts()` gives the store sessions use.
- Not built yet (later tickets): the `posix-rename@openssh.com`
  extension.
- An SCP-only tab's transfers ask (#109). The browser's SCP link is
  `Link::scp(Asking { scp, events, commands, asking })` (private, in
  `browser.rs`): the browser task hands it its event sender and its
  command receiver (behind a `tokio::sync::Mutex`) when it sends
  `TransferOnly` and ends. Its `Connect` opens `Scp::connect()` (attended)
  and forwards `HostKey` / `Prompt` as the browser's events; `answer`,
  `decide_host_key` and `cancel` reach it as for the browser's own login.
  The prompted target is in `Scp`, so the typed password answers the
  password question by itself and only what the device asks is asked.
  Commands sent while nothing was asked are drained before each connect.
  Outcomes: `Cancel` is `ConnectFailure::Declined("the login was
  cancelled")`; a key refused with `Cancel` ends `Declined` with the
  login's own reason (it names the host key); anything else `Failed`.
  The engine makes `Declined` a `TransferError::Declined` (displayed
  bare), which does **not** drop the host's link, so the next waiting
  transfer connects and asks; `Failed` is `TransferError::Connect` as
  before (the link dropped, given again on the next add). One question at
  a time follows from one transfer per host. The engine bounds no connect
  by the stall limit, and the connect timeout pauses while the user is
  asked (`UserWait`), so an open question never fails the transfer. A
  question that goes without an answer (the transfer cancelled, its
  connection given up) sends `BrowserEvent::QuestionClosed` (a drop guard,
  `Open`), and the tab takes its dialog off. `close(id)` (a Reconnect)
  forgets the host's link when it is this browser's
  (`Transfers::forget_scp`, crate-private, `Arc::ptr_eq` on the `Connect`)
  so the waiting ones wait for the next browser and ask over it, and
  cancels the running transfer when its question is open (the slot's
  `asking` flag); `close_host` cancels everything as before. A browser
  closed while its link still connects refuses what is asked from then
  on, with the unattended reasons. The file job keeps `Link::scp(scp)`:
  `Scp`'s own `Connect` is unattended, unchanged.
- Known limits of the tab's transfers (#106): a host renamed while its tab is open
  keeps its transfers under the old id until the tab closes (they are
  closed by the ids the tab opened with); the main window's Destroyed
  closes browsers outside the runtime, at once (the app is ending).
- Tests: `tests/file_browser.rs` (#103: stored login listing folders
  first, a start folder and the fallback with its notice, the path box
  absolute/relative/`..`/missing, path helpers for `/` and `flash:` paths,
  rename/delete/new folder and a rename onto an existing name, bad names,
  host key asked and cancelled, login question answered and cancelled, a
  cut connection then a new browser, closing seen by the server beside a
  terminal, no SFTP, the test shell's `show sftp` / `cut sftp`).
  `tests/file_connection.rs` (stored profile with no shell request,
  one jump host, attended host key and one-time code, a wrong answer,
  unattended refusals naming the key, the question and a jump host's
  password, a refused connection, the hang-up before the banner for a file
  connection and a session, a cut connection noticed and a new one
  working, drop and close closing device and jump host, one channel at a
  time, a terminal session beside it). `tests/sftp.rs` (home, listing,
  links on Unix only, stat, make/rename/delete, rename onto an existing
  name, existing folder, folder not empty, missing files, permission
  denied, a 3 MiB write and read back byte for byte, create truncating, an
  operation after a cut, a 40 MiB read cut half way, no SFTP both ways,
  SFTP after a refusal). `tests/scp.rs` (#104: 3 MiB sent and fetched
  back byte for byte, directly and through a jump host; the fall back
  both ways, holding no connection and connecting per transfer; SFTP kept
  when there; a failed connection not a fall back; two transfers against
  a server that closes after each, and the second on one connection
  failing; each device behaviour in its own words, the IOS and Junos
  hints and no others; a header naming the file without its folder; a
  missing file; a short fetch; a cancelled send; transfer-only refusals;
  plain text refusing SCP after a fall back, as cEOS at privilege 1).
  `tests/transfers.rs` (#105: 6 MiB up and down byte for byte over SFTP
  and SCP with the size checks and hashes against `md5`/`sha2` in the
  test; MD5 and SHA-256 of `abc` against known values; three transfers in
  order with a waiting one removed; two hosts at once; progress while
  held, then the outcome; watches never read; a cancelled SFTP upload
  removed, a refused remove saying the file remains, a cancelled SCP
  upload saying one may remain, cancelled SFTP and SCP downloads leaving
  no local file; a short remote file failing the upload; presence three
  ways; stalls of an SFTP upload, an SFTP download and an SCP fetch after
  3 s; a severed connection failing the running transfer within 5 s and
  the next waiting until `connect`; SCP logging in per transfer and SFTP
  not; waiting without a link; a missing local file; `close`; a refused
  SCP fetch leaving an existing local file as it was; an empty file over
  SCP; an SCP upload stall (accepted as `Stalled` or
  `Lost(NotAnswering)`: the test server's stalled SCP blocks its whole
  connection, keepalives too); a caller's own `Connect` failing, the next
  transfer waiting until `connect`; #109: one declining, that transfer
  failing alone and the next going through on the same link).
  `tests/file_tab_transfers.rs` (#106: uploads into the folder shown in
  order and listed, downloads of names into a folder, names present on
  the device and locally, a running and a waiting upload cancelled with
  what each left, waiting transfers running after a severed connection
  once a new browser connects, `close_host` cancelling and still removing
  the partial file, `follow` slower than the transfer ending on the last
  state, bad names refused, SCP only: `TransferOnly` with the start
  folder, send and fetch by path holding no connection, `present` empty,
  a refusal with its hint, a failed connection not leaving the next send
  waiting, the test shell's `scp off` / `scp on`; #109, SCP only:
  `engineer-otp` asked only the code for each send and going through once
  answered, a cancelled code failing that send alone and the next asking
  again, a key trusted once asked for each send and fetch (refused: that
  one fails naming the key, the next asks), a stored profile with
  remembered keys and a key trusted and remembered asking nothing per
  transfer, a code left open 4 s with a 3 s stall limit still going
  through, a transfer cancelled while asking giving `QuestionClosed`,
  `close(id)` with a question open cancelling it and the next waiting
  for and asking over the new browser, `close_host` with one open).
  `tests/transfer_memory.rs` (alone in its binary: 40 MiB up and down
  over SFTP and SCP while the working set (Windows) or RSS (Linux) is
  sampled; it grew about 10 MiB here, and the test fails above 24 MiB:
  checked by reading the file whole in the window, 46 MiB. It was 16 MiB
  until #114: on Linux the growth is 16 to 18 MiB, glibc's arenas).
  Support: `tests/support/sftp.rs` and the server's `serve_sftp`,
  `refuse_sftp`, `close_sftp_at_once`, and SCP's `serve_scp`,
  `scp_quirk`, `scp_running` (`testing.md`). The example server's
  `--sftp <folder>` serves a folder, for the smoke test later.
- The closing sweep (#108, 2026-11-21): every story of #95 walked, its
  result per story in the closing comment on #95. The bullets after this
  one say what each ticket had verified when it was built: their "not
  verified: the lab devices" is answered here. Changed by the sweep
  besides the IOS-XE fix above: the SCP form's path starts as the
  platform's file folder when the form appears (`joinPath(folder, "")`,
  story 16; it was empty until a file was chosen), and the smoke checks
  it. On the lab through its
  jump host, in the built app driven over DevTools as the smoke does
  (mouse and keys; the file dialog answered on the IPC protocol; a
  workspace of the lab hosts, profiles typed into the memory store):
  - ceos1: a terminal (jump host's and device's keys asked), then Files
    with no question in `/mnt/flash`; a 400 MiB upload in 4 s (the tab
    showed 111 MiB/s) with the terminal echoing in 45-61 ms and answering
    in 46-63 ms meanwhile (47-62 and 62-93 ms before); the device's
    `md5sum` equal to the MD5 shown; the same name again asks to replace
    (the confirm button focused on a lab host, as built); a changed key
    (another key written into `known_hosts`) asked over the tab, Cancel
    focused, the refusal said, then no question once put back.
  - cjunos1: its first-use key asked over the tab; `/var/tmp`; an upload
    cancelled at 119-137 MiB said its partial file was removed, and it
    was; with `sftp-server` deleted the tab shows the SCP form with the
    Junos hint and sends over SCP ("the size the device wrote could not
    be checked"); set again, the listing shows the file and Delete
    removes it without a question.
  - csr1: the SCP form with "it closed the SFTP channel before
    answering", no hint, "No connection held"; `show users` lists only the
    terminal's vty while the tab is open; a fetch of 300 KiB checked; a
    send to `flash:<name>` filled in, `verify /md5` equal to the MD5
    shown; `no ip scp server enable` gives "the device refused:
    Administratively disabled." with the hint, and it sends again once
    back on; opened eight times, the fall back each time.
  - A host with no profile (frr1 as root): username and password asked in
    the tab, then `/root`; a wrong password is "authentication failed".
  - A login typed for an SCP-only host (csr1 with no profile) is asked
    once and **not again per file**: the tab's `Scp` keeps the prompted
    target and connects unattended with it. The spec said it would be
    asked per file; what fails per file is a login question (a one-time
    code) or a key trusted once.
  - Against the example server (connect timeout 3 s): `hold sftp` stalls
    an upload at 2.8 MiB, which fails 8.8 s later with "no byte moved for
    9 s: the device stopped answering", its partial file said to remain,
    the tab lost with Reconnect; a production host's download asks
    nothing and its tab has the mark.
  - A cancelled SCP overwrite on IOS leaves no file: IOS removed the
    partial one, and the old one was gone when the new one started.
  - Not done: a real Explorer drag and the real file dialog (played, as
    in the smoke), a Junos read-only login, an IOS device with its five
    lines in use (both from #96 and the core tests), NX-OS, Linux.
- Verified: everything above against the in-process server on Windows.
  Not verified here: the lab devices (the prototype did that, #96), Linux
  (`npm run test:linux` not run for this ticket), a stalled request's
  timeout (not provoked), a jump host dropping under an open file
  connection, `Lost::NotAnswering` and `Lost::Broken` (only a device's
  disconnect message is tested), and the order in which a drop closes the
  device and its jump hosts (only that both go).
- Verified for #104: `tests/scp.rs` against the in-process server on
  Windows. Not verified: the lab devices with this code (the prototype's
  SCP, which this follows, ran against csr1, cjunos1 and ceos1, #96),
  Linux, a stalled SCP transfer's timeout (not provoked), a typed login
  asked again per transfer (`Scp::connect` is not exercised attended).
- Verified for #105: `tests/transfers.rs` and `tests/transfer_memory.rs`
  against the in-process server on Windows. Not verified: the lab devices
  with the engine (a stall, a cut, a refused remove on cjunos1's
  read-only login), Linux (`npm run test:linux` passes since #114, the
  memory test's limit raised), a jump host
  dropping mid-transfer, an attended `Connect` (only `Scp`'s unattended
  one is exercised), a download whose size differs from SFTP's stat (no
  quirk for it), a local disk that fills.
- Verified for #106: `tests/file_tab_transfers.rs` and the rest of the
  core tests on Windows; the smoke's `fileTransfers` part against the
  example server (SFTP and `--scp-only`) on Windows, with the mouse; the
  system file dialog is answered by intercepting its request on the IPC
  protocol in the page (`plugin:dialog|open` on `fetch`: Tauri's
  `invoke` cannot be replaced), and drops are emitted as Tauri's
  `tauri://drag-*` events, so neither the real dialog nor a real
  Explorer drag is exercised. Not verified: the lab devices (a real IOS
  `flash:` send, cEOS and Junos refusals in the tab), Linux, a jump host
  in the tab, a transfer surviving a Reconnect in the app (core test
  only), the workspace switch question with transfers (no smoke step).
- Verified for #109: `tests/file_tab_transfers.rs`, `tests/transfers.rs`
  and the rest of the core tests on Windows, against the in-process server
  only: no lab device asks a question at login. The smoke's
  `fileTransfers` gained a step for it (a typed `engineer-otp` login on
  the example server's `--scp-only` device; the code asked over the form
  per send, Cancel failing that send, the next answered with the mouse)
  that passes since it was first run on merged main. Its first run failed
  on its own check that the row read Connecting the moment the question
  appeared: the row follows the transfer at most every 200 ms and still
  read Waiting, so the step waits for it. Not verified: a host key question per transfer in
  the app (core test only), a jump host's question per transfer, Linux.
- Verified for #103: the core tests above and the smoke's `filesTab` part
  against the example server on Windows (see `window.md`). Not verified:
  the lab devices (an IOS `flash:` folder's realpath and listing; a device
  without SFTP in the app; jump hosts in the Files tab), Linux.

- Security review (2026-11-22): a local file name from the device goes
  through `local_path(into, name)` in `files/browser.rs` (download, fetch,
  `existing_locally`): refused as `BadName` when it holds a character
  `workspace::forbidden_in_file_names` refuses (control characters and the
  Windows separators and specials), is a Windows device name
  (`is_device_name`), ends in a dot or space, or is not exactly one
  `Component::Normal`. A Unix device can list a name with a backslash,
  which on Windows would leave the folder or name a share. Test:
  `file_tab_transfers.rs` `names_that_cannot_be_files_are_refused_before_anything_is_added`.
