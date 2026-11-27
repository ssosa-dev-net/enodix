# Where enodix keeps your passwords, and what it connects to

This page is for a network engineer deciding whether to put their logins
in enodix, and for the security team that has to approve it. It says
where passwords, key passphrases and named secrets are kept, what is
encrypted and what is not, how scripts get a login, every connection the
app can make and why, and what the app does not protect you from.

Everything here was checked against the source code of the version it
ships with. The paths are those of Windows unless a Linux path is given.
`<data folder>` is `%APPDATA%\dev.enodix.app` and `<local folder>` is
`%LOCALAPPDATA%\dev.enodix.app`; on Linux both are
`~/.local/share/dev.enodix.app`.

## In short

- Logins are kept in Windows Credential Manager, or, if you turn it on, in
  the app's own vault: one file encrypted with a key made from your master
  password. Workspace files never hold a password, only the name of a
  credential profile.
- Session logs, job results, config backups, the workspace, known host keys
  and preferences are plain files. Nothing encrypts them.
- With the update check off, the app connects only where you send it: a
  session, a Files tab, a job, a script or a Python setup you confirmed.
  A local shell is a program the app starts as you, in a pseudo terminal:
  it can do whatever you type, and nothing it does is seen or limited by
  the app. The app gives it only the workspace folder and the headless
  binary's path (`ENODIX_WORKSPACE`, `ENODIX_CLI`) and its own data
  folder (`ENODIX_DATA_DIR`), and writes no log of it unless you ask.
  The update check is the one connection it makes on its own; unsigned
  pre-releases and builds of your own have no release server and check
  nothing. One exception existed up to 0.4.1: if Windows is signed in to
  a Microsoft account, the window's engine, Microsoft's WebView2, now and
  then fetched that account's profile from Microsoft's Office servers a
  few seconds after the app started. It was not caused by typing, and
  nothing seen points to it carrying what you type. The app now starts
  the engine with that sign-in off, and the connection has not been seen
  since; what that rests on, and what is still open, is under "The
  window" below.
- There is no telemetry, no account, no cloud service and no crash
  reporting.
- The app trusts every program that runs under your Windows account, as an
  SSH agent does. It does not protect you from malware running as you.

## Where logins are kept

A **credential profile** holds a username and one way to log in: a
password, a private key file with its passphrase, or the SSH agent. It can
also hold an enable secret. A **named secret** is a name and a text value
(an API token, say) that scripts can ask for. Both belong to one workspace:
the workspace's id (in its `_workspace.yaml`) keeps them apart from every
other workspace's.

The workspace's YAML files hold only a profile's name
(`credential_profile: core-login`). A private key stays in its own file
where you keep it; the profile holds its path and its passphrase, never a
copy of the key.

### Windows Credential Manager (the default)

Each profile is one generic credential named
`dev.enodix.app:<workspace id>:<profile name in lower case>`, and each
named secret one named `dev.enodix.app.secret:<workspace id>:<secret name in
lower case>`. The credential's secret is the profile (or the secret) as
JSON, passwords included. You can see the entries under **Control Panel,
Credential Manager, Windows Credentials**.

They are stored for your account on this computer only: they do not roam
to other computers with a roaming profile. Windows protects them with your
logon, so another Windows user cannot read them, but any program running
as you can.

Credential Manager can lose a change, or hand back the value before it,
when two programs change it at the same moment. Copies of the app and
`enodix-cli` therefore take turns at it (through a named mutex, which
holds nothing), and the app reads every save back before calling it done:
a change Credential Manager did not keep is an error, and nothing is
reported saved. Another program writing Credential Manager at the same
moment can still undo a change a moment later, and the app cannot see
or prevent that.

`enodix-cli clear-keychain` removes every entry named `dev.enodix.app:*` or
`dev.enodix.app.secret:*`; the uninstaller runs it when you tick its data
box.

### The vault (if you turn it on)

**Preferences, Security** turns the vault on. The app then moves every
profile and secret out of Credential Manager into one file,
`<data folder>\credentials.vault`, and deletes them from Credential
Manager once the file has been read back and compared; then it lists
Credential Manager again, and if anything is still there it says so and
does not turn the vault on. If the app stops between writing the file and
deleting the entries (a crash, a power cut), the copies left in Credential
Manager are taken into the vault and deleted the next time you unlock it.
While that file exists the vault is the only store; turning the vault off
moves everything back (an entry too large for Credential Manager, which
holds 2,560 bytes per entry, is named before anything is moved).

Beside the vault is `credentials.vault.lock`, an empty file: copies of the
app take turns through it when they save a profile or secret, so that two
copies saving at the same moment both keep their changes. It holds
nothing, and is made the first time something is saved, with the vault on
or off.

- **Key:** derived from the master password with Argon2id (version 1.3),
  64 MiB of memory, 3 passes, 4 lanes, a 32-byte key, and a random 32-byte
  salt per file. The parameters are written in the file, so a later
  version can raise them; a file asking for more than 4 GiB, or more than
  16 passes or lanes, is refused as damaged. A new master password needs
  at least 8 characters.
- **Encryption:** XChaCha20-Poly1305 with a new random 24-byte nonce each
  time the file is written. The file's header (format, version, key
  parameters and salt) is bound to the encrypted body, so it cannot be
  changed without the body failing to open. A small sealed check tells a
  wrong password from a damaged file; a SHA-256 checksum covers the whole
  file.
- **What is in the clear in the file:** the format, its version, the key
  parameters and the salt. The names of profiles and secrets, the
  usernames and the workspaces they belong to are inside the encrypted
  body.
- **The master password** is not written anywhere. If you forget it, the
  vault cannot be opened: there is no recovery. **Start over** on the
  unlock screen deletes the file, and any copies of profiles or secrets
  still in Credential Manager with it.
- **While unlocked:** you type the master password at start. From then
  until the app is closed, the key and the opened profiles and secrets stay
  in the app's memory; they are wiped when the vault is closed. The app
  does not lock itself again when you are away or the computer locks.
- The file roams with a roaming Windows profile, as the rest of the data
  folder does (it is in `%APPDATA%`). It is encrypted, so that is a copy of
  the encrypted file, not of the passwords.

### Linux

The app has no Linux keychain support yet: the vault is the only store, so
it must be turned on before any profile can be saved. The vault and its
lock file are made readable by you alone (mode 0600) and the data folder
`~/.local/share/dev.enodix.app` yours alone (0700); a vault made by an
earlier version is tightened so when it is unlocked. Linux is a supported
platform; see `docs/guide/linux.md` for what that rests on.

## What is encrypted and what is not

Only the vault is encrypted by the app. Everything else is a plain file,
readable by anyone who can read your files:

| What                                      | Where                                                 | Encrypted                               |
| ----------------------------------------- | ----------------------------------------------------- | --------------------------------------- |
| Profiles and secrets, with the vault      | `<data folder>\credentials.vault`                     | Yes (see above)                         |
| Profiles and secrets, without the vault   | Windows Credential Manager                            | By Windows, for your account            |
| Session logs (on for every host by default) | `<local folder>\logs`                               | No                                      |
| Kept job results (every host's output)    | `<local folder>\jobs`                                 | No                                      |
| Config backups and their history          | `<local folder>\configs`                              | No                                      |
| The workspace (hosts, folders, snippets, web links, jobs) | the folder you chose                  | No                                      |
| Trusted host keys                         | `<data folder>\known_hosts`                           | No (public keys)                        |
| Preferences, the list of workspaces       | `<data folder>\preferences.yaml`, `state.yaml`        | No                                      |
| The Python environment and its choice     | `<local folder>\python`, `<data folder>\python.yaml`  | No                                      |
| Files you download from devices           | where you save them                                   | No                                      |
| The window's web view data                | `<local folder>\EBWebView`                            | No                                      |

Session logs, job results and config backups hold what the devices print.
A configuration can contain secrets (SNMP communities, pre-shared keys,
password hashes), and so can anything shown in a session. Encrypting them
is not planned. A password you type at a device's prompt is not shown on
screen by the device, so it is not in the log.

Telnet sends everything, passwords included, in the clear; a telnet
session is marked unencrypted in its header.

On Windows `<data folder>` is in the roaming part of your profile: in a
domain with roaming profiles it is copied to the profile server, with the
vault and the trusted host keys. Since 0.5.0 job results, config backups
and the Python environment are in `<local folder>`, which stays on the
computer; the first start of 0.5.0 moves them there from `<data folder>`
(the README says how). Before 0.5.0 they roamed too, and a profile server
may still hold a copy of them.

## How a script gets a login

Scripts never need a password written in them. The Python package
`enodix` asks the headless binary `enodix-cli` (`credentials`, `secret`),
which asks the running app over its **credential agent**:

- On Windows, a named pipe `\\.\pipe\enodix-agent-<id>` (the id is made
  from the data folder's path). Only your Windows account may open it (its
  access list grants your account alone), and connections from other
  computers are refused. `enodix-cli` checks that the program serving the
  pipe runs as you before asking it anything, so a program of another user
  that took the name first gets nothing. On a computer shared with other
  users (a terminal server, a jump box), such a program can still keep
  scripts from getting logins this way, until the app is started again.
- On Linux, a socket `<data folder>/agent.sock` that only you may open
  (mode 0600), and the app checks that whoever connects is you.

The agent only answers: nothing can be listed, stored or changed through
it. If the app is not running, or its vault is still locked,
`enodix-cli` reads Credential Manager
itself, or opens the vault after asking for the master password on the
terminal (never on its command line or from the environment); with no
terminal, as in a job the app runs, a locked vault is refused.

The login travels from the app to `enodix-cli` over the pipe and from
`enodix-cli` to the script over its output pipe. It is not written to disk
and not put in the environment; the buffers that held it are wiped. Once
the script has it, it is in the script's memory (and in Nornir's host
objects, with the Nornir plugin) like any login your own code holds.

When the app runs a script as a job, it gives it these environment
variables, none of which holds a secret: `ENODIX_WORKSPACE` (the workspace
folder), `ENODIX_CLI` (where `enodix-cli` is), `ENODIX_DATA_DIR` (so
`enodix-cli` finds this app's agent), `ENODIX_JOB_TARGETS` (a temporary
file listing the job's hosts, removed when the job ends) and, for **Fill in
from devices**, `ENODIX_FACTS_FILE`. `enodix-cli` then hands out only the
logins of the job's hosts. That limit guards against mistakes; it is not a
sandbox: a script can drop the variable, and any program running as you
can ask the agent for any login of any workspace while the app runs.

A script's own connections check the device's host key against the keys
the app trusts when the Python package sets them up: through a tunnel, and
for a host reached directly too (Netmiko, scrapli, NAPALM's
Netmiko-based drivers). **Fill in from devices** sends no login to a host
whose key the app has not trusted yet. The Nornir plugin still connects to
such a host with its library's own check (Netmiko's accepts any key) and
names it in a warning; connect to each host once in the app, and trust and
remember its key, before a script logs in to it.

## Every connection the app can make

| Connection                             | Who starts it                                                         | Where to                                                       |
| -------------------------------------- | --------------------------------------------------------------------- | -------------------------------------------------------------- |
| The update check                       | The app at start, if **Check for a new version at start** is on; you, with **Check now** | The release server's `latest.json`  |
| Installing an update                   | You, with **Install and restart**                                     | The release server                                             |
| An SSH or telnet session               | You                                                                   | The host's address and port                                    |
| A jump host                            | You, by opening a session through it                                  | Each jump host in turn; the device through the last one        |
| Back up on login                       | You, by logging in to a host that has it                              | The same device (a second channel or connection)               |
| A Files tab, a file job                | You                                                                   | The host over SSH (SFTP, or SCP)                               |
| A job (snippet, config backup)         | You, with **Start**                                                   | Each host of the job                                           |
| A script job, **Fill in from devices** | You, with **Start**                                                   | Whatever the script connects to (your code)                    |
| A tunnel for a script                  | A script, through `enodix-cli tunnel`                                | Listens on `127.0.0.1`; reaches the device through the jump hosts |
| A port forward                         | You, by opening a session to a host that sets `forwards`             | Listens on `127.0.0.1` (or `::1`) only; reaches what the device reaches |
| The Python setup                       | You, after confirming its list                                        | The package index, only for libraries you ticked               |
| A web link, a release's page           | You, with a click                                                     | Your web browser opens it, not the app                         |
| The SSH agent                          | You, with a profile that uses it                                      | The agent on this computer (a pipe, not the network)           |
| Serial                                 | You                                                                   | The COM port, not the network                                  |

Names (a host's address written as a name) are looked up through the
system's resolver when a connection is made.

### The update check

It is the only connection the app makes on its own. It is one HTTPS GET of
the release server's `latest.json`, with `Accept: application/json` and no
other header of the app's: no user agent, no cookie, no identifier, nothing
about you, your computer or your inventory. The server sees what any
connection shows: your public address and the time. It goes through the
system's proxy, if one is set, and Windows checks the server's certificate
against its own certificate store (Windows may fetch the certificate's
revocation status from its issuer to do so).

- It runs once at start, and only if **Check for a new version at start**
  is on (Preferences; kept as `check_for_updates` in `preferences.yaml`).
  **Check now** runs it when you ask, whatever the setting.
- Only a build made by the release script with a release server's address
  knows one. Development builds and unsigned pre-releases (the releases so
  far) have none and check nothing, unless the
  environment variable `ENODIX_UPDATE_ENDPOINT` names one (the tests use
  it). That address must be HTTPS, or plain HTTP to this computer.
- Nothing is downloaded until you click **Install and restart**, and only
  an installed copy offers that. The updater then fetches `latest.json`
  again (its user agent names the updater and its version), downloads the
  installer, checks its signature against the release key and runs it. A
  portable copy, and every copy on Linux, only shows the release's page,
  which opens in your browser.

### Tunnels for scripts

`enodix-cli tunnel` (which the Python package and its Nornir plugin start
for hosts behind jump hosts) listens on `127.0.0.1` only, on a port the
system picks, one per host and per device port the script names. It
connects to the jump hosts first and to the device only when a program
connects to its port. While it runs, **any program on this computer can
connect to those ports**, as with `ssh -L`, including programs of another
user on a shared computer (a jump box with several people logged in). The
device still asks for its own login on its SSH port; a service you named
with `--port` (an HTTPS API, say) is reached as if from the last jump host.
The tunnel ends when the script that started it ends. The jump hosts'
logins stay in the tunnel's memory meanwhile.

### Port forwards

A host's `forwards` (in the workspace files) open with each SSH session to it
and close when it ends; **Stop** in the header's **Forwarding** list closes one
sooner. Each listens on `127.0.0.1` (or `::1`) only: a forward in the
workspace that names any other address, `*` included, is refused as a problem
of its file, so a shared workspace cannot open this computer's ports to the
network. Nothing reaches
the device until a program connects; then the device opens the connection, as
`ssh -L` and `ssh -D` do. While a forward is open, **any program on this
computer can connect to its port**, and reach through the device whatever the
device can reach. A workspace from someone else can set forwards: read it
first (below).

### The window

The interface is a web page drawn by Microsoft Edge WebView2. The page
and its typeface ship inside the app; its content security policy lets it
load nothing but the app's own files and talk to nothing but the app.

Left to itself WebView2 connects to Microsoft. Watching the app's
connections showed it:

- fetch Edge's configuration from `config.edge.skype.com` (sending a
  client id, the Windows version and the install date);
- look up the name `wpad` on the local network, for a proxy script;
- ask `edge.microsoft.com` for component updates a minute after start, and
  download them;
- run Edge's autofill and its text predictions (writing suggestions),
  which work by sending the text being typed to Microsoft;
- fetch the profile of the Microsoft account Windows is signed in with
  from `substrate.office.com`, on Microsoft's Office and Exchange Online
  addresses, now and then a few seconds to half a minute after the
  window opens, whatever you do in it (see "The sign-in with your
  Windows account" below).

The app turns the first four off: WebView2 is started with
`--disable-background-networking`, `--no-proxy-server` and
`--disable-component-update` (and SmartScreen off), its general autofill
is off, and every field of the interface, the terminals' input included,
is marked `writingsuggestions="false"`. The first three were each seen
to stop the connection they name. The last one is turned off since
0.5.0 by switching off Edge's sign-in with the Windows account (see
below).

One thing the engine still does needs no connection: when it resolves a
name, the app's own `tauri.localhost` included, it asks Windows whether
IPv6 reaches the internet by pointing a UDP socket at a fixed Microsoft
address and reading back which of the computer's addresses Windows
picked. Pointing a UDP socket sends nothing, and the end-to-end test
fails if the engine's network log ever shows a datagram sent.

#### The app's own commands, and ports 80 and 443 of your computer

The interface is a page at the address `tauri.localhost`, and it talks
to the rest of the app by commands sent to the address `ipc.localhost`.
Both names mean this computer, and WebView2 answers both itself, from
the app: nothing of that is meant to touch the network. Two things do:

- For each new window WebView2 opens a connection to this computer
  ahead of time, with nothing on it.
- A command still on its way when its window closes or reloads is let
  through to the network once the window is gone. The command has run in
  the app by then; what goes out is a second copy.

Up to 0.4.1 both addresses were `http://`, so that second copy went to
port 80 of your computer in clear, with its contents. Usually nothing
listens there and it was lost. But a program that does listen on port 80
(a web server, a development tool, on a shared computer another user's
program) received it. Measured on 2026-11-22: sixteen commands in one
run of the end-to-end test, all about windows closing and sessions
moving between windows, none carrying a password. Which command is on
its way when a window goes is a matter of timing, so that is no promise
about what could have been in one.

Since 0.5.0 both addresses are `https://`. What slips out now goes to
port 443 as the opening of an encrypted connection, and WebView2 sends
nothing more unless the listener proves it is `ipc.localhost` with a
certificate your Windows trusts, which another program cannot have
without an administrator having installed one. The end-to-end test
plays that other program on every run: it listens on port 80, where a
single byte fails the run, and runs a web server on port 443 with a
certificate of its own making for the app's names, where a completed
connection or a request fails the run. In the first run, port 80 got
nothing and port 443 got 92 connection attempts, none completed.

Not covered: a computer where an administrator has installed a
certificate authority that issues a certificate for `ipc.localhost` to
a local program (some corporate inspection tools issue certificates on
the fly, but for traffic that leaves the computer, which this does not).

WebView2 belongs to Windows and is updated by Windows (Edge's updater),
not by the app, so a later WebView2 may add something new, or stop
honouring a switch; the smoke test watches for it (see the end of this
page).

#### The sign-in with your Windows account

Earlier versions of this page said that WebView2 reached Microsoft's
Office servers while you type. Measured again (#135), typing has nothing
to do with it: it came as often with nothing typed (7 of 8 starts of the
app) as while typing (7 of 8), at most once per start, at a time
unrelated to the keyboard. What it is, as far as could be seen without
breaking the encryption:

- It is made by WebView2's main process with Microsoft's sign-in library
  (OneAuth, which ships inside WebView2) and Windows' account broker, not
  by the part of WebView2 that loads pages: WebView2's own network log
  shows nothing of it. OneAuth was loaded in every start that made the
  connection (5 of 5 checked) and in none that did not (39 checked).
- Its addresses are the ones `substrate.office.com` resolves to (the same
  front door as `outlook.office.com`). WebView2 holds the addresses of
  Substrate's profile and picture services for the signed-in account.
- Within a second of each such connection (5 times), OneAuth rewrote its
  record of the account's profile and picture in
  `%LOCALAPPDATA%\Microsoft\OneAuth`, and not after any of the 52 starts
  without one that were watched for it. That record is shared by every
  Microsoft program that signs in with the Windows account (Office,
  Edge, Teams), which is most likely why the connection does not come at
  every start: after a refresh, starts mostly fetched nothing for a
  while (20 minutes to over an hour).
- What it carries could not be read. Everything seen points to a request
  for your own account's profile, sent with your account's sign-in
  token; nothing points to text you typed being in it.

It was seen on a Windows account signed in to a personal Microsoft
account. A Windows account with no Microsoft account has no profile to
fetch, and there the connection does not happen: on a clean Windows with
a local account only (Windows Sandbox, the same WebView2 version) two
whole runs of the end-to-end test made no connection beyond the
computer, with the network open. Other app makers have reported the
same behaviour of WebView2 157 to Microsoft (WebView2Feedback issue
5760), with no answer so far.

**Turned off since 0.5.0.** The app starts WebView2 with Edge's OneAuth
and implicit sign-in features off (`msOneAuthWAM`, `msForceOneAuthWAM`,
`msOneAuthIDHRD`, `msLoadOneAuthInBackground`, `msImplicitSignin`,
`msImplicitSignInNetworkRetry`, `msM365LinksImplicitSignin`,
`msSeamlessWebToBrowserSignIn`, `msEnableWebToBrowserSignIn`). With
them off the connection was never seen and OneAuth was never loaded.
What that rests on, on a Windows signed in to a Microsoft account:

- Fourteen runs of the end-to-end test with the features off (about
  seven starts of the app in each whole run; eight ran to the end, six
  were stopped part-way by other failures) made no such connection.
- Four whole runs without them made six, nine, six and nine. Two of
  those were made straight after a series of runs with the features
  off, on the same build with only that changed.
- Earlier, starts with and without the features off were run back to
  back twice while OneAuth's record was stale: the start without made
  the connection, the start with did not.
- The implicit sign-in features alone are not enough (one start with
  only those off made the connection).

Whether the connection comes at all depends on OneAuth's shared record
(in the first hour of testing it came at 14 of 16 starts with nothing
turned off, later at 4 of 34), which is why the runs without the
features were made right beside the runs with them. Since 2026-11-22
the end-to-end test fails on such a connection, as on any other. One
thing is open: with the features off, WebView2 loads a
different part of Windows' account broker, so Edge might sign in another
way, through Windows' own broker process, which the app's watch does not
see. That would be Windows signing in to its own account, as it does for
every program, and not the app's engine reaching out.

If you want more than the app's own switch, outside the app, all
untested here: use a Windows
account that is not signed in to a Microsoft account; or have an
administrator block `msedgewebview2.exe` from 40.96.0.0/13 and
52.96.0.0/14 on port 443 in the firewall (this also cuts other programs
that use WebView2 with your account, such as the new Outlook, off from
Exchange Online). Edge's own policies (for example `BrowserSignin`,
`ImplicitSignInEnabled`) are not read by WebView2, and were reported not
to stop it.

### The installer

The installer needs WebView2. Windows 11 has it; if it is missing, the
installer downloads Microsoft's WebView2 bootstrapper from
`go.microsoft.com` and runs it, which downloads WebView2. The portable copy
downloads nothing and does not start without WebView2.

### The Python setup

Python is optional, and nothing is installed before you confirm the
setup's list. The app's own package comes from the wheel shipped beside the
app, installed with `pip --no-index` (no network). Only libraries you tick
(Nornir, Netmiko, scrapli, NAPALM) come from the package index, through
`pip` with your `pip` configuration (its index, its proxy). Choosing your
own interpreter installs nothing.

## What the app never does

- It sends no telemetry, usage statistics or crash reports, and has no
  analytics library.
- It has no account, no sign-in, no licence check and no cloud service.
  Nothing is synced anywhere.
- It never uploads your inventory, your logins, session output or
  configs. Device output leaves the computer only if you copy it, or a
  script of yours sends it.
- It opens no port reachable from the network. The only listening sockets
  are the credential agent's pipe (your account only), a script's tunnel
  ports and the port forwards of open sessions (all on `127.0.0.1`; a forward
  may use `::1`).
- It does not forward X11 to devices, nor ports from a device back to this
  computer (`ssh -R`). It forwards your SSH agent only to a host whose
  **Agent forwarding** setting asks for it (off by default) and only once
  you have said yes on this computer, in the session's pane before it logs
  in: for that session, or always for that host at that address. It shows
  **Agent forwarded** in the session header while it is.
- It does not open web pages itself: web links go to your browser.

## What it does not protect against

- **A program running as you.** Any program you run can read your
  Credential Manager entries, ask the credential agent for any login
  while the app runs, read the app's memory, change your workspace or its
  data folder, and set environment variables the app or WebView2 reads
  when it starts. The vault keeps the passwords encrypted on disk, not
  from a program that runs while it is unlocked.
- **Plain files.** Session logs, job results and config backups are as
  readable as any file of yours: on a shared or synced disk, in a backup,
  on a roaming profile's server. So is a workspace in a shared folder or a
  git repository (it holds no passwords, but it does hold addresses, web
  links and jump hosts).
- **A workspace from someone else.** Its web links can point anywhere and
  send a host's address and other fields to that address when you click
  them. Its jump hosts are machines your sessions pass through, and they
  get the jump host's login. Its port forwards open ports on this computer
  while a session to the host is open. Its **Agent forwarding** setting
  only asks: a session to such a host asks you before the login whether to
  forward your SSH agent, and nothing is forwarded unless you say so on
  this computer (an "always" is kept in the app's data folder, never in the
  workspace, and covers that workspace, host, address and port only). Its scripts are code that runs as you, and its
  platform definitions say what is sent at login (paging off, privileged
  mode, the show command of back up on login; each must be one line, and a
  definition whose command holds a line break is refused). Its hosts'
  addresses decide where your logins go. Read a workspace you did not
  write before you use it, as you would a script.
- **Files from a device.** A file is downloaded under the name the device
  gives it, so a name that could leave the folder you chose (`..\`, a
  drive, `\\server`) or that Windows cannot hold is refused: rename it on
  the device first.
- **Tunnel and forward ports.** While a script's tunnel runs, or a
  session's port forward is open, anyone on this computer can use its ports
  (see above).
- **A device or jump host you trusted wrongly.** The first connection
  shows the host key's fingerprint and asks; trusting a key you did not
  check accepts whoever answered. A `@cert-authority` line you add to the
  known hosts trusts, without asking, every host certificate that authority
  signs for the hosts its patterns name: anyone holding its private key can
  stand in for those hosts. A `@revoked` line refuses its key (a host key,
  a certificate's key or an authority) for the hosts it names.
- **TLS through a tunnel.** The app checks no TLS certificate of a device;
  a script that calls an HTTPS API through a tunnel must check it itself.
- **A forgotten master password.** There is no recovery.

## How this page is kept true

The end-to-end smoke test (`scripts/smoke.mjs`) runs the built app through
everything it can do: sessions, jump hosts, files, jobs, scripts, the
Python setup, the vault, restarts. All along it watches the connections of
the app's processes (the app, WebView2, the Python it runs and
`enodix-cli`) and the names WebView2 looks up, and fails if any of them
reaches anything but the test's own servers on this computer.
