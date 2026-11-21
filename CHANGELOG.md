# Changelog

What each release added, newest first. The numbers in brackets are tickets
in the project's tracker.

## Unreleased

### New

- A device's files, in the app [95]. **Files** on an SSH host (its details
  sheet, the tree's context menu, a session's menu) opens a tab that logs
  in with the host's credential profile, through its jump hosts, on a
  connection of its own: an open terminal on the device is not slowed. A
  login that must be typed and an unknown or changed host key are asked in
  the tab. [docs/files.md](docs/files.md) describes it, what each platform
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

- Upgrading removes the Python wheel of the version it replaces, and
  uninstalling removes every wheel and then the program folder [111].
  Before, each upgrade left the old wheel behind, and the uninstaller the
  folder with it.
- A running `netterm-cli.exe` (a script using it) is closed like the app
  before an upgrade or an uninstall, asked about first when the installer
  has a window [111]. Before, an upgrade kept the old one beside the new
  app, and an uninstall left it.
- The uninstaller's data box says what it deletes ("Also delete settings,
  saved logins and the vault, config backups, kept job results, session
  logs and the Python environment") and now removes the saved logins from
  Windows Credential Manager too, with the new `netterm-cli clear-keychain`
  [111]. Workspace folders are never touched.
- The licence is installed beside the program and shown as a page of the
  installer. The installer, both programs and the Installed apps entry name
  the publisher and carry a copyright line; `netterm-cli.exe` has version
  details [111]. The installer remembers its folder under
  `HKCU\Software\Sergio Sosa\netterm`; upgrading moves it from
  `HKCU\Software\netterm`.
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
- **Not solved yet:** while text is typed, the engine now and then
  connects to Microsoft's Office servers. What these connections carry has
  not been seen. Until this is closed, assume typed text may reach
  Microsoft; `docs/trust.md` has what is known [135].
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
- A job's command timeout counts from the device's last output, so a long
  configuration that is still printing after 60 seconds is waited for; a
  command whose output never stops is ended after 30 minutes [122].
- A device that asks "Password:" again right after the login fails the
  host as a login question. Before, a job typed `enable` and then the
  enable secret into it [122].
- Closing netterm while jobs run asks first and names them, then cancels
  them as Cancel does. A script job's processes and the tunnels they hold
  end with the app, also when it crashes or is killed, and so does a Python
  setup in progress. Before, a script went on working against devices
  with nothing left to stop it. The kept run says "Cancelled: the app was
  closed while the job ran." [124]
- A script job no longer leaves running what it started once it exits
  [124].
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
- Saving a host in the app no longer writes over a change made to its file
  outside the app (a pull, an editor) that the app had not loaded yet, and
  no longer brings back a host deleted outside. The edit is refused with
  the reason, and can be made again once the workspace has reloaded [117].
- Planning a job on thousands of hosts in one folder, and exporting them,
  no longer slows down with the square of their number: 20 000 hosts plan
  in under half a second, not more than four [117].
- A second copy of netterm notices when the first turns the vault on or
  off or changes its master password, and says so. Before, it went on
  saving to a keychain nobody read, or called the vault damaged [117].
- Two copies writing the vault at once no longer share one temporary file,
  and a temporary file left by a write that was cut is removed at the
  next start [117]. Still open: two copies saving at the same moment can
  lose one of the changes.
- A host or settings file that starts with a byte order mark, as some
  Windows editors write it, loads [117].
- More names Windows reserves are refused for hosts and folders: `COM0`,
  `LPT0`, `COM¹` to `COM³`, `LPT¹` to `LPT³`, `CONIN$`, `CONOUT$` [117].
- `netterm-cli` accepts `--` before values that start with `-` (a named
  secret called `-ro`, a folder `-old`), and `--option=<value>` [127].
- `netterm-cli clear-keychain` removes only with `--yes`; without it, it
  says what it would remove and removes nothing. The uninstaller passes it
  [127].
- Where nobody can type the vault's master password (a scheduled task
  while nobody is logged on), `netterm-cli` says so at once. Before, it
  waited for ever [127].
- Python: a telnet host behind a jump host is marked `behind_jump_host`
  and gets a `tunnel_error`, as the README said. The package warns once
  (`netterm.VersionWarning`) when the binary is another version [127].
- On Linux, a script asks the credential agent only through a socket that
  is this user's and closed to others [127]. The .deb, the .rpm and the
  AppImage carry the licence [127].
- The Python setup's confirmation says that pip also installs what each
  package needs, unlisted and unpinned. The README says how to install on
  a computer with neither WebView2 nor internet. The note on back up on
  login mentions the second login a device that refuses a second channel
  gets [127].

## 0.4.1

A pre-release: not code-signed, and without automatic updates (install it
over 0.4.0 by hand).

### New

- Scripts reach more than a device's SSH port through the app's jump
  hosts. Name the ports when the tunnels open (the `ports` argument of
  `netterm.tunnels`, the Nornir plugin's `tunnel_ports` option, or
  `netterm-cli tunnel --port 830`) and NETCONF, eAPI, RESTCONF or any
  other TCP service of the device goes through the same jump chain and
  jump connection as the SSH port. `netterm.endpoint(host, port, tunnels)`
  and the Nornir host data's `netterm.ports` say where to connect for a
  port of a host, behind a jump host or not, so one script runs in both
  cases. ncclient and scrapli-netconf check the device's host key
  against the keys the app trusts (`netterm.host_key_options`); the app
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
[docs/interface.md](docs/interface.md) says where each one now is.

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

- `netterm-cli`, a headless binary installed beside the app: the resolved
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
