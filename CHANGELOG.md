# Changelog

What each release added, newest first. The numbers in brackets are tickets
in the project's tracker.

## Unreleased

### Fixed

- Documentation that no longer matched the app: which workspaces the
  credential agent answers for, the update check of signed releases, telnet
  through a jump host in the lab guide, automatic reconnect, the 1.25 MiB a
  background tab takes before its device is paused, and the date the device
  platforms were last tested. The release steps now include checking
  `docs/trust.md` against the code [78].

### For contributors

- The interface has tests of its own, run by `npm run test:ui` (vitest),
  `npm run check` and CI: every way to paste asks first where it should,
  where terminal input comes from, shortcut keys on AZERTY, Dvorak and
  other layouts, the multi-send group's pastes, and a session's first
  output before it has opened [71].
- The window's side of the app (the shell) has tests of its own: each kind
  of session (SSH, telnet, serial, a local shell, the system ssh tab) is
  opened through the same commands the window uses, without a window, and
  must answer in time; and a check of the shell's source fails on any call
  that would lock the workspace again while it is held, the fault behind
  0.8.0's system ssh tab. They run on Linux and Windows with the other
  tests [70].

## 0.8.1

A fix release for two faults in 0.8.0.

### Fixed

- **Open with the system ssh (insecure)** never opened: the tab stayed on
  connecting and the workspace stopped responding until the app was
  restarted. It now opens [27].
- A local shell tab could open with an empty terminal and no prompt, most
  often when opened right after the app started on Windows. Its start-up
  answer to the console is no longer lost, so the prompt always appears [22].

## 0.8.0

This release reaches more of what a network engineer touches in a day: port
forwards and a SOCKS proxy through a device, a shell on your own computer in
a tab, snippets that wait for the device, files over a serial console, and a
long list of SSH additions (certificates, hardware keys, agent forwarding,
telnet behind a jump host, hosts from your `~/.ssh/config`). Devices that
speak only old algorithms the app will not implement can now be opened with
the computer's own `ssh`, marked insecure. Linux is stated as a supported
platform.

### New

- Snippets with steps: lines sent one at a time to the session in front of
  you, over SSH, telnet or serial, each waiting for the device's prompt (or
  a pattern of its own) with a timeout; questions such as `[confirm]`
  answered when they show; the run stopped at the first error pattern,
  naming the step and line. Shown before it runs (always on production),
  followed in a bar with Stop while it runs, and stopped by typing in the
  terminal. Written beside the other snippets in `_snippets.yaml` [23].
  A paste, or a snippet without steps, also stops a run, and none of that
  paste is sent to the session.
- Port forwards and a SOCKS proxy on SSH sessions, as `ssh -L` and `ssh -D`
  do [21]. Set `forwards` per host or per folder in the workspace files; the
  app opens them when the session connects, over the session's own
  connection (so through its jump hosts, with no second login), and closes
  them when it ends. The header shows **Forwarding n**; clicking it lists
  every open forward, where it listens and why its last connection failed,
  with **Stop** for each. Remote forwards (`ssh -R`) are not supported.
- **Local shell**: a shell on your own computer in a tab, a split pane or a
  session window, with no workspace needed [22]. On Windows PowerShell,
  Command Prompt and each WSL distribution; on Linux your login shell and the
  others in `/etc/shells`. Choose it in the quick connect form or with
  **New local shell** in the palette; the **Local shell** preference picks
  the default. It starts in the workspace's folder and has
  `ENODIX_WORKSPACE` and `ENODIX_CLI` set, so the Python bridge works in it.
  It is not logged and never joins multi-send.
- Settings per host or folder, inherited like the others [24]: **terminal
  type** (`terminal_type`: `xterm-256color`, `xterm`, `vt100`, `ansi` or
  `linux`), **character encoding** (`encoding`: `utf-8`, `iso-8859-1`,
  `windows-1252` or `cp437`; the terminal and the log always show UTF-8),
  **timestamps in session logs** (`log_timestamps`, the local time before
  each line) and **automatic reconnect** (`auto_reconnect`). A session that
  drops waits 2 s, growing to 60 s over 8 tries; the closed bar counts it
  down, any key stops it and Enter reconnects at once. It tries again only
  when the connection was lost, or the device (or a jump host) could not be
  reached or hung up before the login began. It never repeats a refused
  login, a device that closes or stops answering during the login, or a jump
  host lost while a login runs through it, because each try can count against
  the account on a RADIUS or TACACS+ server; reconnect by hand once you have
  checked the login. It never answers a host-key question. The count starts
  again only after a session has stayed connected for a minute, so a device
  that accepts and drops at once uses the tries up. A telnet session behind a
  jump host that goes away is a lost connection and reconnects; one the
  device closes does not.
- **Colour schemes** in Preferences: Solarized Dark and Light, Dracula, Nord
  and Gruvbox Dark besides the app's own colours, and **Own colours** for the
  background, text, cursor, selection and any of the 16 palette colours [24].
  Highlight rules that name a palette colour follow them; rules with a
  `#rrggbb` colour keep it.
- **Key bindings** in Preferences [24]: move the window's shortcuts (search,
  switch workspace, close tab, multi-send group, open in window, side view,
  Preferences, edit host, details), and bind a key to a snippet with
  **Snippet keys**, sent to the active session like a click in the Snippets
  view; that key no longer reaches the device. A key needs Ctrl or Alt
  (function keys stand alone). Keys the app keeps (copy and paste, search, the
  tab keys, the font size keys, the side view keys) cannot be bound, and
  Preferences says why. A key bound to a snippet sends it once per press.
- **Send break** on telnet sessions, in the session menu [24]: the telnet
  BREAK command, which a terminal server turns into a break on its console
  line (serial sessions already had it).
- **Send file…** and **Receive file…** on serial sessions, with XMODEM,
  XMODEM-CRC, XMODEM-1K and YMODEM [47]: the way to load an image into a
  router's ROMMON or a switch's boot loader before the network is up. A bar
  shows the file, bytes, blocks and retries, and **Cancel** tells the device
  to stop. A damaged block is sent again up to 10 times; a device silent for
  a minute at the start, or 10 seconds later, ends the transfer with the
  reason. YMODEM names never reach outside the chosen folder and never
  replace a file (`config.txt` becomes `config (2).txt`); a file cut short is
  removed. ZMODEM and Kermit are not supported, nor transfers over telnet
  or SSH (the Files tab does SSH).
- **OpenSSH user certificates** [26]: a profile with a private key offers the
  certificate beside it (`id_ed25519-cert.pub`) first, as `ssh` does, and a
  profile that uses the SSH agent uses the certificates the agent holds. If
  the server refuses the certificate, the key is tried on its own.
- **Host certificates** [26]: a `@cert-authority` line in `known_hosts`
  trusts the authority that signs your devices' keys instead of each key.
  The patterns follow OpenSSH (`*`, `?`, `!`, `[host]:port`, and hashed
  patterns from `ssh-keygen -H`). The certificate must be a host
  certificate, signed by that authority, valid now, without critical
  options, and name the host as written in the workspace; otherwise the
  app asks about the key inside it and says why the certificate was not
  taken. Jobs count such a host as trusted; Python scripts do not use
  authorities yet.
- **Revoked keys** [26]: a `@revoked` line refuses a key, a host certificate's
  key or an authority, with no question, even for a key trusted before.
  `known_hosts` may start with a byte-order mark.
- **Hardware security keys** (`ed25519-sk`, `ecdsa-sk`) work through the SSH
  agent: add the key to the agent and touch it when asked [26]. The app
  cannot read a `*_sk` key file itself.
- **Agent forwarding** [26]: set `agent_forwarding: true` on a host or
  folder, and a terminal session to it asks, in its pane before it logs in,
  whether to let the device use your SSH agent: **Connect without
  forwarding** (the default), **Forward for this session**, or **Always
  forward to this host**. The workspace only asks; the answer is yours, on
  this computer. "Always" is kept in `agent_forwarding.json` in the app's
  data folder, never in the workspace, and covers that workspace, host,
  address and port only. The header shows **Agent forwarded** while it is on.
  The agent goes to the device only, never to a jump host, and never in
  jobs, file connections, back up on login, script tunnels or quick
  connections.
- **A password after a key** [26]: a server that wants both
  (`AuthenticationMethods publickey,password`) is logged in to with the
  profile's key or agent key, and the pane then asks for the password.
- **Telnet through a jump host** [26]: a telnet host with a jump host goes
  through the jump host's SSH connection, so only the last leg is in the
  clear (the **Unencrypted** marker stays). Jobs and scripts do not reach such
  hosts yet.
- **Import from SSH config…** reads an OpenSSH client config (`~/.ssh/config`
  by default) and previews the hosts it would create [26]. `HostName`,
  `Port` and a `ProxyJump` to another host of the same file are kept;
  `User`, `IdentityFile`, `Match`, `Include`, several hops and
  `ProxyCommand` are not, and the preview lists each.
- **Devices that offer only algorithms the app will not implement** [27]:
  when a session fails for that reason, its message has **Open with the
  system ssh (insecure)**, which opens a tab running the computer's own
  `ssh` with the outdated algorithms allowed and an **Insecure** marker in
  the header. It is not an enodix SSH session: no host key check against the
  app's list, no session log, no jump hosts, and you type the password in
  the terminal. What it reaches depends on the client, because a current
  OpenSSH has dropped some of these. With the two clients tried (OpenSSH for
  Windows 9.5, and OpenSSH 10.6 from Git for Windows), both reached CBC and
  3DES ciphers, SHA-1 and MD5 MACs, the SHA-1 key exchanges and `ssh-rsa`,
  `ssh-dss` only with 9.5; none reached Blowfish, CAST or RC4 ciphers or a
  RIPEMD MAC. The app asks the client what it knows and passes only that. See `docs/guide/legacy-crypto.md`.
- Linux is stated as a supported platform, with what that rests on and what
  is not yet checked [25]: see `docs/guide/linux.md`.

### Changed

- App shortcuts match the key that prints the letter on your layout (on
  AZERTY the key labelled W for Ctrl+Shift+W, on Dvorak the key labelled K
  for Ctrl+K), and also the key at that place on a QWERTY keyboard. In
  0.7.1 either match ran the shortcut; now, when the two name different
  actions, the printed letter wins and one press never runs two things [24].

### Security

For anyone who opens a workspace someone else wrote:

- Port forwards listen on `127.0.0.1` or `::1` only. A workspace that names
  any other address, `*` included, has a problem on the file and opens
  nothing. While a forward is open, any program on this computer can use its
  port.
- A workspace can ask to forward your SSH agent but cannot do it: you
  approve on this computer, per session or always for that host, and an
  "always" is not stored in the workspace.
- A revoked key is refused everywhere: sessions, jobs, tunnels, the
  pre-flight check and scripts, which are never handed a revoked key to
  accept. A revoked authority does not vouch for a host.
- The system `ssh` tab accepts the weaknesses of the algorithms the device
  forces, and the app does not check its host key. Use it only for devices
  you cannot update.

## 0.7.1

A distribution release: the app is the same as 0.7.0. The Python bridge is
now on PyPI, and releases go to winget.

### New

- The Python bridge is on PyPI (`pip install "enodix==<version>"`), the
  same wheel the app ships [19]. Pin it to your app's version
  (`enodix-cli --version`); a different version warns when it runs.
- New releases go to winget (`winget install ssosa-dev-net.enodix`) once
  its first version is accepted [19].

### Fixed

- The wheel's metadata listed a classifier PyPI does not accept, so it
  could not be uploaded there [19].

## 0.7.0

Hosts are now edited in a tab of their own, with Save and Revert, and a
change reaches the host's file (and git) only when you save it. This
release also has the new enodix icon and fixes from an internal security
review.

### New

- **Edit host** (the details sheet, the palette, Ctrl+Shift+E) opens a host
  editor tab beside the terminals [10]. It follows the selected host or can
  be pinned to one. Edits collect until **Save** (Ctrl+S), which writes
  only the fields you changed, or **Revert**; the tab title counts them.
  Switching host, closing the tab, quitting and opening another workspace
  ask Save / Discard / Cancel.
- Inherited settings show where they come from ("Inherited from
  Europe/Madrid") with **Override**, and the ones a host sets itself come
  first, with "Show N inherited settings" for the rest. A port or timeout
  the app would refuse is explained under its field as you type.
- The details sheet is a short summary of the host, with its actions in
  three menus (Open, Run, More) that work from the keyboard.
- Snippets are edited in a tab: at least 82 columns, line numbers, no
  wrapping, and the variables the snippet uses beside it. Closing a snippet
  tab with unsaved changes asks first, as the host editor does [12].
- The enodix logo, as the app icon at every size [16].

### Changed

- Buttons, inputs and menus share two heights, headings and labels have a
  clearer scale, and every text colour passes 4.5:1 contrast in both
  themes.
- The current search match in a dark terminal is readable.
- **Fill in from devices** and the Nornir plugin check the host key of a
  device reached directly, not only through a tunnel, against the keys the
  app trusts [6]. Fill in from devices refuses a host whose key is not
  trusted yet; the Nornir plugin warns about it.

### Fixed

- The Save / Discard / Cancel question for unsaved host edits stayed on
  screen after any answer [12].
- Snippet text no longer touches the line numbers, also when scrolled
  sideways [13, 14].

### Security

From an internal review [5]:

- A download whose name would leave the chosen folder on Windows (`..\`, a
  drive, a `\\server` path), or that Windows cannot hold, is refused.
- A platform definition with a line break in `disable_paging`,
  `privilege_command` or `show_config` is an error and is not used.
- A workspace cannot replace the `enodix` Python package that **Fill in
  from devices** runs, and the Python package never looks in the current
  folder for `enodix-cli`.
- A host name that would match other hosts in `known_hosts` (a comma, a
  wildcard) is not remembered.
- New vault master passwords need at least 8 characters; on Unix the vault
  and its folder are readable by their owner only.
- Links printed in a terminal open nothing.

## 0.6.0

The first release of enodix on GitHub, and the first with automatic
updates: installed copies (the Windows installer's) check for a newer
release and install it when you say so; the portable copy and the Linux
packages only tell you a new version is out. The installer is not
code-signed, so Windows SmartScreen may warn before it runs: choose
**More info**, then **Run anyway**. Check the download against
`SHA256SUMS.txt` below.

The app is renamed **enodix** (it was netterm): the program, the
`enodix-cli` command, the `enodix` Python package and every `ENODIX_*`
environment variable. It is installed as a new app beside netterm and
starts with an empty data folder; a vault file copied over from netterm
still opens with its master password.

Every installer and package now carries `THIRD-PARTY-NOTICES.txt` beside
the licence: each component of other people's that the program contains,
its licence and the licence text. Contributions come under the
Contributor Licence Agreement in `CONTRIBUTING.md`.

## 0.5.0

A pre-release: not code-signed, and without automatic updates (install it
over 0.4.1 by hand).

**If you run an earlier version, read "What the app connects to" below.**
Up to 0.4.1 one of the app's own internal commands could be sent in clear to
port 80 of your own computer when a window closed, where another program
on that computer could receive it; and on a Windows signed in to a
Microsoft account the window's engine connected to Microsoft's Office
servers. Both are closed in this version.

On Windows the first start moves config backups, kept job results and the
app's Python environment to `%LOCALAPPDATA%`, once (see "Windows
installer").

### New

- A device's files, in the app [95]. **Files** on an SSH host (its details
  sheet, the tree's context menu, a session's menu) opens a tab that logs
  in with the host's credential profile, through its jump hosts, on a
  connection of its own: an open terminal on the device is not slowed. A
  login that must be typed and an unknown or changed host key are asked in
  the tab. [docs/guide/files.md](docs/guide/files.md) describes it, what each platform
  needs and its limits.
  - A device with SFTP gets a browser: folders and files with size, time
    and permissions, a path box, upload (a file dialog or files dropped
    from Explorer), download into a folder you pick, rename, delete and
    new folder. It starts in the folder where files belong on the host's
    platform.
  - A device with SCP only (IOS and IOS-XE; a Junos device whose SFTP
    server is off) gets a form: send a file to a path, fetch a path. The
    app tries SFTP and falls back by itself, connects for each file and
    holds no connection in between, so the tab does not take one of an IOS
    device's few lines. A refusal is shown in the device's own words, with
    the command that turns the feature on where the app knows it. What a
    device asks at each of those logins (a one-time code, a host key
    trusted once) is asked in the tab for each file; Cancel fails that
    file alone [109].
  - Transfers show progress and speed and can be cancelled, one at a time
    per host with the rest waiting. Sizes are checked where the protocol
    allows, a cancelled or failed upload removes its partial file where it
    can and says so where it cannot, a transfer that stops moving fails
    with a reason, and an upload's MD5 and SHA-256 are shown to compare
    with the device's own verify command.
  - On a production host, uploading, deleting and renaming ask first.
    Replacing a file asks wherever the device can say the file is there;
    the SCP form says that it cannot.
- **Send file…** on a folder, a saved filter or a host sends one file to
  many devices as a job [107]: a list beforehand of the hosts it will skip
  and why, three devices at once unless changed, each device's progress,
  and a kept run with the file's name, size, MD5 and SHA-256 and what
  happened on each device. A device that already has the file (same name,
  same size) counts as done; one with another size, or one that cannot say
  (SCP only), fails unless **Overwrite** is ticked.
- Several hosts can be selected in the Hosts view and a job run on them
  [110]: Ctrl+click adds a host or takes it out, Shift+click selects the
  hosts between, as the tree shows them. The view counts them, with a
  button to clear them (Escape too), and the context menu on one of them
  offers the jobs only: Run snippet…, Run script…, Send file…, Back up
  configs and Fill in from devices, each on exactly those hosts. Such a
  job cannot be saved as a job, as before for more than one host.
- A platform definition has a seventh value, the folder where files belong
  on that platform [102]: `flash:` for IOS, `/mnt/flash` for EOS,
  `/var/tmp` for Junos, `bootflash:` for NX-OS (not verified), none for SR
  Linux and FRR. The Files tab starts there and the file job sends there.

Checked on the lab's devices through its jump host: SFTP on Nokia SR Linux
24.10.1, Arista cEOS 4.36.0.1F, FRR 10.2.1 and Juniper cJunos Evolved
25.4R1.13, SCP on Cisco IOS-XE 17.03.08 (CSR 1000v).

### Windows installer

- Config backups, kept job results and the app's Python environment
  move from `%APPDATA%` (roaming, copied at every logon in a domain) to
  `%LOCALAPPDATA%`, once, at the first start of this version [128].
  Preferences, the list of workspaces, the vault and the trusted host
  keys stay where they were. On one drive the move is a rename; across
  drives it is a checked copy that an interruption leaves whole in one
  place. If both folders already hold data, neither is touched and the
  main window says so. A Python environment on another drive is not
  copied: it keeps working and moves when it is set up again.
- Upgrading removes the Python wheel of the version it replaces, and
  uninstalling removes every wheel and then the program folder [111].
  Before, each upgrade left the old wheel behind, and the uninstaller the
  folder with it.
- A running `enodix-cli.exe` (a script using it) is closed like the app
  before an upgrade or an uninstall, asked about first when the installer
  has a window [111]. Before, an upgrade kept the old one beside the new
  app, and an uninstall left it.
- The uninstaller's data box says what it deletes ("Also delete settings,
  saved logins and the vault, config backups, kept job results, session
  logs and the Python environment") and now removes the saved logins from
  Windows Credential Manager too, with the new `enodix-cli clear-keychain`
  [111]. Workspace folders are never touched.
- The licence is installed beside the program and shown as a page of the
  installer. The installer, both programs and the Installed apps entry name
  the publisher and carry a copyright line; `enodix-cli.exe` has version
  details [111]. The installer remembers its folder under
  `HKCU\Software\Sergio Sosa\enodix`; upgrading moves it from
  `HKCU\Software\enodix`.
- The README says how to uninstall, what stays, and the installer's silent
  switches [111].

### The first run

The path a newcomer takes was walked in the built app with an empty data
folder, and what stalled it was fixed [116].

- A connection that fails says what to check next: the address, the port,
  whether the device is up, the login and where it is set, the SSH agent.
  A jump host's failure says so for the jump host.
- A folder or a file that is not there is said plainly, without the
  system's "os error 2"; so is a file chosen where a folder was meant.
  Creating a workspace over a folder with files says to open it. The
  workspace switcher says "Open an existing folder", as the welcome view
  does.
- New host and a changed address refuse an address written with its port
  (`10.0.0.1:2222`) and say where the port goes. Before, it was accepted
  and failed as a name that could not be resolved.
- A jump host named by its host name where its id belongs gets the id in
  the error. A CSV file with no row of column names is told to get one.
- The empty pane says a double click connects a host. The login prompt,
  the Credential profile setting, the Credentials view and the reports of
  the CSV and Nornir imports say how a host gets a credential profile.
- Opening a folder that holds other things and no host file (a Documents
  folder picked by mistake) asks before making it a workspace, and nothing
  is written into it until the answer is yes [131]. Before, the app wrote
  its `_workspace.yaml` there and showed every subfolder as a host folder.
  A folder with host files, a workspace's own files or nothing in it opens
  as before. `enodix-cli inventory` never wrote into such a folder and
  now warns that it is not a workspace.
- The CSV and Nornir imports check each address as New host does [131]:
  one with a space in it fails its row; in a CSV, so does one with a port
  after it, and the reason says where the port goes; a Nornir hostname
  with a port gives the host that port. New host refuses a space inside an
  address too. "Import 0 hosts" can no longer be pressed.

### SSH login banners

- The notice a device sends before the login (`banner login` on IOS,
  `/etc/issue.net` on Linux) is shown: it is the first thing in the
  terminal, before any password or code question, on every connection,
  also when the login fails, and the session log keeps it [134]. Before,
  it was dropped. A jump host's banner comes first, under a grey line
  naming the jump host. Jobs, config backups and the Files tab leave it
  out. Checked on the lab's IOS-XE and Arista cEOS devices, which now
  carry awkward banners all the time (lines ending in `#` and `>`,
  "Password:" in a sentence), over SSH and, on cEOS, telnet.

### What the app connects to

- New: [docs/trust.md](docs/trust.md) says where passwords, key
  passphrases and named secrets are kept, what is encrypted and what is
  not, how a script gets a login, every connection the app can make and
  why, and what the app does not protect against [115].
- The window's engine, Microsoft's WebView2, no longer contacts Microsoft
  on its own [115]. In every earlier release it fetched Edge's
  configuration at start (with a client id, the OS version and the install
  date), looked up `wpad` for a proxy script, asked for component updates
  and downloaded them, and ran Edge's autofill and writing suggestions on
  what is typed. The app's own code made none of these connections; they
  were found by watching every connection of the app's processes through a
  whole run of the end-to-end test, which now fails on any of them.
- **The app's own commands could reach port 80 of your computer in
  clear, in every version up to 0.4.1** [142]. The interface talks to the
  rest of the app by commands the engine is meant to hand over directly.
  A command still on its way when its window closed or reloaded was also
  sent out as an ordinary web request to port 80 of the same computer,
  with its contents. Usually nothing listens there and it was lost, but
  a program that does (a web server, and on a shared computer another
  user's program) received it. In a test run sixteen commands went that
  way, all about windows closing and sessions moving between windows,
  none with a password; which command is on its way when a window goes
  is a matter of timing. The app now addresses its pages and commands
  with `https`: what slips out goes to port 443 as the opening of an
  encrypted connection, and nothing follows unless the listener has a
  certificate Windows trusts for the app's name. The end-to-end test
  plays that other program on both ports on every run.
- On a computer where Windows is signed in to a Microsoft account, the
  engine also connected to Microsoft's Office servers a few seconds
  after the app started: its own sign-in library fetching that account's
  profile and picture. Typing did not cause it, and nothing points to
  typed text being in it, though what it carried was not read. The app
  now starts the engine with Edge's sign-in with the Windows account
  off [135]. Fourteen test runs with it off made no such connection;
  four without it made six, nine, six and nine, two of them straight
  after runs with it off. On a Windows with no Microsoft account the
  app never made the connection at all [139]. The end-to-end test now
  fails on such a connection. `docs/trust.md` has what this rests on
  and what stays unknown.
- The README said the app made no connection you did not start, and that
  session logs could be written to a folder you chose. Neither was true;
  both are corrected [115].

### Fixes

Found by deliberately hostile use of the inventory, the stored logins and
sessions [117] [118], and by a review of the 0.2.0 work against its specs
[112].

- Start in a job tab runs exactly the plan that tab shows [121]. Before,
  the app kept only the plan made last, so with two job tabs at pre-flight
  Start in one could run the other's job: on production hosts too, with no
  question asked for them. The app itself now also refuses to start a plan
  whose production hosts were not confirmed for it. A pre-flight that is
  no longer current says so and offers "Plan again".
- Multi-send: a paste or a snippet begun in a tab outside the group stays
  in that tab, even if the tab joins the group while it is asked about or
  still being sent [123]. Before, its remaining lines went to every
  member, production ones included, without the group's question.
- Multi-send: a group paste goes to the group as it is when you press
  Send. If the group changes while you are asked, in any window, the
  question closes, nothing is pasted and the window says so; a member that
  leaves or closes during a paste gets no more of it; a paste from a tab
  whose group was ended elsewhere is called off and said, no longer
  dropped silently [123].
- Multi-send: the app itself, not only the window, refuses to add a
  production session to the group without its question [123].
- A job whose `enable` is answered with a message and leaves the device
  at the same prompt (IOS's "% No password set") fails the host with the
  device's words [122]. Before, it carried on unprivileged, and a config
  backup could keep the device's error message as the configuration. A
  backup that shows nothing but an error is never kept.
- A job no longer takes text that looks like the device's prompt for the
  end of a command [136]. Before, output that ended a line in `#` or `>`
  (a table's `total#`, a description) and then paused for a third of a
  second was taken for the prompt: the job typed its next line into the
  answer and closed the session, the rest of the output was lost, the host
  still ended Done, and a config backup cut there could be kept as a
  version. A command now ends when the device comes back to the prompt it
  was typed at; a prompt that changes on purpose (enable, a configuration
  mode, a new host name) is waited for a little longer; a configuration
  ends only at the prompt it was asked at.
- A job keeps at most 32 MiB of each host's output, and of a script's, and
  says where it stopped [119]. Before, a device or a script that flooded
  output was held whole in memory and in the kept run. A config backup
  whose output was cut keeps nothing.
- A job on hundreds of hosts no longer spends most of its time writing its
  kept run, and its window keeps up with it: 100 hosts at 50 at once took
  5 to 21 seconds and now take about 3 [119].
- A device that echoes the enable secret no longer shows it in a job's
  output or its kept run [119].
- Two hosts whose files differ only in their extension (`r1.yaml`,
  `r1.yml`) each keep their own output in a kept run [127].
- Fill in from devices no longer loses every host's facts on Windows when
  another program reads the facts file at the moment it is written [119].
- A job's command timeout counts from the device's last output, so a long
  configuration that is still printing after 60 seconds is waited for; a
  command whose output never stops is ended after 30 minutes [122].
- A device that asks "Password:" again right after the login fails the
  host as a login question. Before, a job typed `enable` and then the
  enable secret into it [122].
- Closing enodix while jobs run asks first and names them, then cancels
  them as Cancel does. A script job's processes and the tunnels they hold
  end with the app, also when it crashes or is killed, and so does a Python
  setup in progress. Before, a script went on working against devices
  with nothing left to stop it. The kept run says "Cancelled: the app was
  closed while the job ran." [124]
- A script job no longer leaves running what it started once it exits
  [124].
- Reconnecting a session (Enter, or Reconnect in its menu) uses the host
  as it is now: its address, port, jump host and other settings, and its
  credential profile as the Credentials view has it [130]. Correct a
  password or a port and press Enter. Before, a reconnect used what the
  first attempt had, so only connecting again from the tree picked up a
  correction. A typed login is used again unless the host has a profile
  now; a host whose profile is gone asks for the login; a host that was
  deleted, or set to another protocol, is said so and not tried. Quick
  connections reconnect as they were made.
- A telnet session over a link that went silent ends as lost within about
  three connect timeouts, as an SSH session does [133]. When the device
  has been quiet, the session asks it for a timing mark; a device that
  never answers one is left alone and never taken for dead. Before, such
  a session stayed "Connected" until it was closed by hand.
- A jump host that stops answering while a session is being opened is
  named in the failure. Before, the device was reported as not answering
  [133].
- A session whose output was held back when its window went away ends
  and lets go of the device (and of a serial port). Before, it stayed
  connected for ever [118].
- A device that hangs up during the login is no longer reported as a
  refused password: the message says the device closed the connection, or
  why the connection broke, and names the jump host when it was a jump
  host's connection that ended [118].
- Telnet: a device that negotiates options without end can no longer make
  the app's memory grow, or keep it answering the same refused option
  [118].
- Session logs and job output: a control sequence that never ends no
  longer makes memory grow [118].
- Reconnecting a session in place sends the terminal's size once the new
  connection is up. Before, a resize at that moment could reach the old
  connection, and the device kept the wrong size.
- On Linux, the AppImage found no Python: its runtime's `PYTHONHOME` and
  library paths reached the system's Python. Programs the app starts no
  longer inherit them [114].
- On Linux, a copy installed from the .deb or the .rpm is told to update
  with the package manager, not to unpack the new version over itself.
  The packages' menu entry has a category [114].
- The Hosts view says when the workspace folder cannot be reached (a
  network drive that disconnected, a folder moved or removed). Nothing can
  be saved meanwhile, and the workspace loads again by itself once the
  folder is back [132]. Before, live reload stopped for good after the
  folder was removed and made again, and nothing said so. A folder that
  comes back as another workspace is not taken for the one that was open.
- A host or folder name written in its other Unicode form (`café` with a
  combining accent, as macOS writes it) is the same name. Two such files
  side by side both load and are shown as a problem [132].
- A workspace file nested deeper than 32 levels is refused at once. Before,
  a file made that way took seconds or minutes to refuse [132].
- A live reload of a very large workspace no longer holds up the window:
  with 20 000 hosts it held the workspace for 2.1 s, now for 27 ms [132].
- Saving a host in the app no longer writes over a change made to its file
  outside the app (a pull, an editor) that the app had not loaded yet, and
  no longer brings back a host deleted outside. The edit is refused with
  the reason, and can be made again once the workspace has reloaded [117].
- Planning a job on thousands of hosts in one folder, and exporting them,
  no longer slows down with the square of their number: 20 000 hosts plan
  in under half a second, not more than four [117].
- A second copy of enodix notices when the first turns the vault on or
  off or changes its master password, and says so. Before, it went on
  saving to a keychain nobody read, or called the vault damaged [117].
- Saved logins in Windows Credential Manager: Windows loses changes, or
  hands back the value from before them, when two programs change the
  credentials at the same moment, whatever the entries are called. Copies
  of the app and `enodix-cli` now take turns at it, and every save and
  delete is read back before it is called done; a change Windows did not
  keep is an error, never a save reported done [138]. A program that is
  not enodix writing at the same moment can still undo a change a moment
  later; Windows gives no way to prevent that.
- Two copies of enodix saving to the vault at the same moment both keep
  every change: they take turns through a lock file beside the vault, and
  two turning the vault on at once make one vault [125]. Before, about
  half of such changes were lost without a word. A temporary file left by
  a write that was cut is removed at a later start [117].
- Turning the vault on checks that Windows Credential Manager is empty
  afterwards and says so if it is not. Copies a crash in the middle left
  there are taken into the vault and removed at the next unlock, and Start
  over removes them too, so no profile comes back after it [125].
- Turning the vault off names an entry too large for Credential Manager
  before anything is moved [125].
- A copy starting, or `enodix-cli` reading, while another copy writes the
  vault no longer takes the vault for missing: on Windows the file's name
  can be gone for tens of milliseconds while it is replaced. A save waits
  for a program that holds the file for a moment (an antivirus scanner)
  and no longer fails [125].
- The vault's plain contents are wiped from memory after turning on,
  after each write and after turning off, where copies were left before
  [125]. `enodix-cli` says a damaged vault is damaged before it asks for
  the master password [125].
- A host or settings file that starts with a byte order mark, as some
  Windows editors write it, loads [117].
- More names Windows reserves are refused for hosts and folders: `COM0`,
  `LPT0`, `COM¹` to `COM³`, `LPT¹` to `LPT³`, `CONIN$`, `CONOUT$` [117].
- `enodix-cli` accepts `--` before values that start with `-` (a named
  secret called `-ro`, a folder `-old`), and `--option=<value>` [127].
- `enodix-cli clear-keychain` removes only with `--yes`; without it, it
  says what it would remove and removes nothing. The uninstaller passes it
  [127].
- Where nobody can type the vault's master password (a scheduled task
  while nobody is logged on), `enodix-cli` says so at once. Before, it
  waited for ever [127].
- Python: a telnet host behind a jump host is marked `behind_jump_host`
  and gets a `tunnel_error`, as the README said. The package warns once
  (`enodix.VersionWarning`) when the binary is another version [127].
- On Linux, a script asks the credential agent only through a socket that
  is this user's and closed to others [127]. The .deb, the .rpm and the
  AppImage carry the licence [127].
- The Python setup's confirmation says that pip also installs what each
  package needs, unlisted and unpinned. The README says how to install on
  a computer with neither WebView2 nor internet. The note on back up on
  login mentions the second login a device that refuses a second channel
  gets [127].
- A saved job no longer keeps the answer to a variable whose name looks
  like a secret (`{{enable_password}}`, by the rule the imports use): it
  is asked at each run, and the save form says so [126]. Before, a
  password typed as an answer went into `_jobs.yaml`, which is shared
  through git. Such an answer already in the file is never used or shown;
  the saved job names it so it can be removed from the file by hand (the
  app cannot remove it from the git history).
- `enodix-cli.exe` starts on a Windows that has no Visual C++ runtime
  (a freshly installed Windows) [139]. Before, it did not start there at
  all, in 0.4.1 too: script jobs, the Python package and the
  uninstaller's removal of saved logins failed on such a computer. Found
  by running the installer check in Windows Sandbox.
- A file job no longer sends a file that changed after pre-flight [137]:
  the host is sent nothing and says the file changed. A file that changes
  while it is being sent fails the host, and what was sent stays on the
  device. Before, each host got whatever the file was at that moment and
  the run reported the planned size.
- Cancelling a file job, or closing the app, while a device has stopped
  answering ends in about 2 s [137]. Before, it waited for the device up
  to the connect timeout and more. The host then says a partial file may
  remain on the device.
- The Jobs view lists thousands of kept runs several times faster (5,000
  runs: 0.83 s before, 0.15 s now, 0.085 s when listed again) [137].
- In split panes, keys typed just after moving to another pane could
  reach the previous pane's device [141]. Clicking a terminal in one
  pane and then one in another within a moment, or a tab and then
  another pane's terminal, let the first answer from the core hand the
  keyboard back. It needed a busy computer to show. A focus the window
  gives a moment later now never takes the keys from where they have
  gone since.
- Linux: a serial port without modem control lines (a pseudo terminal,
  some adapters) opens; before, raising RTS failed the whole open
  [58]. A file manager or browser opened from the AppImage no longer
  gets the AppImage's environment, which made it load the bundle's
  libraries [58]. The AppImage starts dark on a desktop whose dark look
  is a style, such as XFCE's Adwaita-dark; switching the style while it
  runs needs a restart there [143].
- A connection that times out says where it stopped [140]: no TCP
  connection to the address and port (and through which jump host), no
  SSH identification line, the key exchange not finished, no answer to
  the login, to the channel request or to the SFTP request, or no shell.
  A jump host's failure names the jump host and its stage. Before, each
  of these read "the device did not answer within N s". Jobs and the
  Files tab show the same words.
- The window's Minimize, Maximize and Close stay inside a narrow
  window [120]. Before, with the side view open and the window narrow,
  they ran past its edge. The side view and the other panes give way
  first now.
- A session window that stops answering for 10 s is named in the main
  window, which offers to bring its sessions back (still connected,
  without their earlier output) or to wait [120]. Before, the sessions
  were stuck in it. If the web engine ends as a whole, the app ends too;
  before, it stayed open with blank windows that could not be closed.
- Two hosts whose files differ only in the extension (`r1.yaml` and
  `r1.yml`) keep a config history each [127]. Before, they shared one.
  History they already shared is left where it was and reported, since
  the versions cannot be told apart.
- A job's pre-flight names the hosts whose host key, the device's or a
  jump host's on the way, is not trusted yet, says how to trust it and
  counts them in its summary [129]. Before, such a host was only refused
  once the job ran. It looks in the known host keys and connects to
  nothing, so a key that has changed still shows only when the job runs.

## 0.4.1

A pre-release: not code-signed, and without automatic updates (install it
over 0.4.0 by hand).

### New

- Scripts reach more than a device's SSH port through the app's jump
  hosts. Name the ports when the tunnels open (the `ports` argument of
  `enodix.tunnels`, the Nornir plugin's `tunnel_ports` option, or
  `enodix-cli tunnel --port 830`) and NETCONF, eAPI, RESTCONF or any
  other TCP service of the device goes through the same jump chain and
  jump connection as the SSH port. `enodix.endpoint(host, port, tunnels)`
  and the Nornir host data's `enodix.ports` say where to connect for a
  port of a host, behind a jump host or not, so one script runs in both
  cases. ncclient and scrapli-netconf check the device's host key
  against the keys the app trusts (`enodix.host_key_options`); the app
  does not check TLS certificates (the README says what that means).
  Checked through the lab's jump host: NETCONF on cJunos Evolved's port
  830 and eAPI on cEOS's port 443.

### Fixes

- Renaming or moving a host, or a folder holding hosts, keeps what
  belongs to them [99]. Their session logs and config history move to
  the new name (a session open on the host goes on logging there), and
  the jump host settings and saved jobs that named them are changed to
  the new name, so the hosts behind a renamed bastion still connect
  through it. Only the files that named them are written, and only that
  value. History that cannot move, because an earlier host of the new
  name left its own there, stays where it was and is never mixed with it;
  a notice in the Hosts view says what stayed and where.
- The buttons of a question drawn over a terminal take the mouse again:
  the host-key question (Trust once, Trust and remember, Cancel), a
  server's login question, and the cards after a failed connection. A
  click went to the terminal underneath, so the question could only be
  answered with the keyboard.
- Clicking a side view's icon in the rail, or a pane's Split right, Split
  down, Close pane or multi-send button, leaves the keyboard with the
  terminal: typing after the click went nowhere.
- After clicking the search bar's arrows, Enter goes on finding and
  Escape closes the search; Escape works from any of the bar's controls.
- After a failed negotiation or a failed agent login, the card over the
  terminal is the only "New connection": the closed bar under its
  backdrop showed one that could not be clicked.
- A saved filter's chip, once applied, stays readable under the mouse.
- In a narrow pane at the window's top-right, the pane's buttons (new
  connection, multi-send, split right and down, close pane) fold into a
  "…" button that lists them. They were cut off under the window's
  buttons and could not be used, already at 800 px with two panes side by
  side or two tabs in one pane, in session windows too.

## 0.4.0

A pre-release: not code-signed, and without automatic updates (install it
over 0.3.0 by hand).

### Terminals in windows of their own

For troubleshooting across several monitors (spec #81, #83 to #88).

- **Open in window** on a session's menu, its tab's menu and Ctrl+Shift+N
  moves a session to a window of its own, still connected, with its
  scrollback and screen. Output arriving during the move appears once and
  in order (#84).
- A session window holds several sessions in tabs and split panes; a
  session can move into a window already open, and Dock back returns one
  or all of them to the main window (#85).
- What protects you follows the session: the production colour and mark,
  the unencrypted and legacy-crypto markers, paste confirmation, host-key
  and login questions, logging (#86).
- Multi-send spans windows: one group for the app, shown in every window
  (#87).
- Closing a session window with live sessions asks whether to dock them
  back or close them. Closing the main window now asks once when a session
  is live in any window; before, it closed without asking (#88).
- The palette (Ctrl+K) works in a session window (#88).
- A session window whose page crashes (or whose renderer is ended from
  the Task Manager) closes, and the main window takes its sessions, still
  connected, saying their earlier output was lost; a flood going on keeps
  flowing (#89).
- A new session window opens beside the window it came from on monitors
  with different display scaling too; before, from a monitor scaled more
  than the primary (a laptop screen at 150% beside an external monitor at
  100%), it could open on the other monitor. Near a monitor's edges it is
  fitted so that what is seen stays inside the screen (#90).
- Known limits: a session window whose page hangs without crashing, or
  the web engine crashing as a whole, still leaves sessions stuck until
  the app is closed; the crash handling is not yet tried on a Linux
  desktop (#58).

### Other changes

- The app draws its own window frame: the system title bar is gone, the
  tab bar drags the window and holds Minimize, Maximize and Close (#80).
  The Windows 11 snap-layouts flyout on the Maximize button is not kept.
- The details sheet's actions are one row of icons with tooltips, in place
  of a block of text buttons.
- With details open, the problems line no longer sits between the tree and
  the sheet.
- A tab bar with more tabs than fit has no scrollbar: the mouse wheel moves
  it, and the shown tab stays in sight.

## 0.3.0

A pre-release: not code-signed, and without automatic updates (install it
over 0.2.0 by hand).

### A new interface

The same features, reached differently (spec #61, #62 to #79);
[docs/guide/interface.md](docs/guide/interface.md) says where each one now is.

- A new look: one dark and one light palette, the Inter typeface for the
  interface (terminals keep your font), real icons, all shipped inside the
  app. Text meets WCAG AA contrast in both themes (#62, #79).
- An activity rail switches one side view between Hosts, Snippets, Jobs,
  Config backups and Credentials, and folds it away. The toolbar and the
  right-hand properties column are gone; host and folder details open as
  a sheet when asked for (#63 to #66, #70).
- Each terminal has one quiet header line with its markers and a menu of
  its actions, also on the tab; a status line shows the vault and the
  terminal size (#67).
- Job runs, past runs, config history, Preferences, the CSV and Nornir
  imports, platform definitions, highlight rules, the fill-in review and
  Python setup open as tabs beside the terminals, and can be split into a
  pane of their own (#68 to #73).
- Questions about one terminal (host key, login, paste confirmation, a
  snippet's variables, closing a live tab, joining multi-send) cover only
  its pane; the snippet editor is part of the Snippets view (#74, #79).
- A welcome view with recent workspaces and quick connect, and empty
  views that say what goes there (#75).
- The palette (Ctrl+K) reaches every view and action that moved (#76).
- A job run and a host's config history can open in a window of their
  own, on any monitor, and dock back (#77, #78).
- Preferences and workspace files carry over unchanged; the right panel's
  width and folding are no longer used.

## 0.2.0

A pre-release, like the ones before it: not code-signed, and without
automatic updates (install the next version over it by hand).

### Multi-send

- Type into a group of terminals at once, key by key, so tab completion and
  `?` help work. Tabs join the group one at a time or a pane at a time, and
  each member is marked (#34, #35).
- Joining a production device asks first. A paste or a snippet sent to the
  group is confirmed once, and each member gets its own host's values (#36).

### Credentials

- An optional vault: the app's own encrypted file behind a master password,
  asked once at start. Turning it on moves the profiles out of the system
  keychain, and turning it off moves them back. The password can be
  changed; a forgotten one cannot be recovered (#38, #39).
- Named secrets per workspace, for scripts and integrations (#37).
- The running app hands logins to the headless binary over a local pipe, so
  scripts never read them from disk or the environment (#49).

### Jobs and config backup

- `platform` is an inherited setting, so a folder of identical devices
  needs one assignment (#40).
- Platform definitions, editable in the app, with SR Linux, EOS, FRR, IOS,
  NX-OS and Junos built in (#41).
- Run a snippet on a folder, a saved filter or one host. A list beforehand
  shows the hosts that will be skipped and why, and production targets are
  confirmed. Nothing is asked during the run (#42, #43).
- Runs are kept: a jobs view with search, a diff between hosts or runs,
  save to a folder, delete and run again (#44).
- Config backup as a job, with a history per host and a diff between any
  two versions. A version is saved only when the config changed (#45).
- Back up on login: the config is fetched over a second channel of the same
  SSH connection each time a session opens (#46).
- Saved jobs, kept in the workspace (#53).

### Python

- `enodix-cli`, a headless binary installed beside the app: the resolved
  inventory as JSON, a host's login, a named secret (#48, #49).
- A Python package that presents a workspace as a Nornir inventory with the
  logins of its credential profiles (#50).
- Python setup in Preferences: an environment the app creates after showing
  what it will install, or your own interpreter (#51).
- Script jobs: run a `.py` file of the workspace for a folder, filter or
  host, with its output in the jobs view (#52).
- Fill in from devices: vendor, model, serial number and OS version fetched
  with NAPALM and reviewed before anything is written (#55).
- Scripts reach hosts behind jump hosts through loopback tunnels the binary
  opens with the app's own jump chains (#60).

### Linux

- The core, the headless binary and the Python bridge pass their tests on
  Linux (#56).
- .deb, .rpm and AppImage packages. On Linux the vault is the only
  credential store, and the update check only announces (#57).

### Not verified yet

- The app on a real Linux desktop: only the build and the tests under WSL
  were run.
- The NX-OS platform definition: no device image was available.
- An enable password question on a real device.
- Fill in from devices through the lab's jump host.

## 0.1.2

- The side panels can be resized and hidden.

## 0.1.1

- The workspace toolbar wraps instead of cutting off its buttons.

## 0.1.0

- The first release: SSH, telnet and serial sessions, the inventory as YAML
  files with inherited settings, credential profiles, jump hosts, host-key
  checking, legacy crypto, tabs and split panes, session logs, keyword
  highlighting, snippets, paste safety, web links, several workspaces, CSV
  and Nornir imports, and the update check.
