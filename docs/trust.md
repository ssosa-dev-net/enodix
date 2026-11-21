# Where netterm keeps your passwords, and what it connects to

This page is for a network engineer deciding whether to put their logins
in netterm, and for the security team that has to approve it. It says
where passwords, key passphrases and named secrets are kept, what is
encrypted and what is not, how scripts get a login, every connection the
app can make and why, and what the app does not protect you from.

Everything here was checked against the source code of the version it
ships with. The paths are those of Windows unless a Linux path is given.
`<data folder>` is `%APPDATA%\dev.netterm.app` and `<local folder>` is
`%LOCALAPPDATA%\dev.netterm.app`; on Linux both are
`~/.local/share/dev.netterm.app`.

## In short

- Logins are kept in Windows Credential Manager, or, if you turn it on, in
  the app's own vault: one file encrypted with a key made from your master
  password. Workspace files never hold a password, only the name of a
  credential profile.
- Session logs, job results, config backups, the workspace, known host keys
  and preferences are plain files. Nothing encrypts them.
- With the update check off, the app connects only where you send it: a
  session, a Files tab, a job, a script or a Python setup you confirmed.
  The update check is the one connection it makes on its own; unsigned
  pre-releases and builds of your own have no release server and check
  nothing. One exception is not solved yet: the window's engine,
  Microsoft's WebView2, now and then reaches Microsoft's Office servers
  while you type (see "The window" below).
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
`dev.netterm.app:<workspace id>:<profile name in lower case>`, and each
named secret one named `dev.netterm.app.secret:<workspace id>:<secret name in
lower case>`. The credential's secret is the profile (or the secret) as
JSON, passwords included. You can see the entries under **Control Panel,
Credential Manager, Windows Credentials**.

They are stored for your account on this computer only: they do not roam
to other computers with a roaming profile. Windows protects them with your
logon, so another Windows user cannot read them, but any program running
as you can.

`netterm-cli clear-keychain` removes every entry named `dev.netterm.app:*` or
`dev.netterm.app.secret:*`; the uninstaller runs it when you tick its data
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
  version can raise them; a file asking for more than 4 GiB is refused as
  damaged.
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
it must be turned on before any profile can be saved. (The Linux build has
not been checked on a real desktop yet; see `docs/linux.md`.)

## What is encrypted and what is not

Only the vault is encrypted by the app. Everything else is a plain file,
readable by anyone who can read your files:

| What                                      | Where                                                 | Encrypted                               |
| ----------------------------------------- | ----------------------------------------------------- | --------------------------------------- |
| Profiles and secrets, with the vault      | `<data folder>\credentials.vault`                     | Yes (see above)                         |
| Profiles and secrets, without the vault   | Windows Credential Manager                            | By Windows, for your account            |
| Session logs (on for every host by default) | `<local folder>\logs`                               | No                                      |
| Kept job results (every host's output)    | `<data folder>\jobs`                                  | No                                      |
| Config backups and their history          | `<data folder>\configs`                               | No                                      |
| The workspace (hosts, folders, snippets, web links, jobs) | the folder you chose                  | No                                      |
| Trusted host keys                         | `<data folder>\known_hosts`                           | No (public keys)                        |
| Preferences, the list of workspaces       | `<data folder>\preferences.yaml`, `state.yaml`        | No                                      |
| The Python environment and its choice     | `<data folder>\python`, `python.yaml`                 | No                                      |
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
domain with roaming profiles, job results and config backups are copied to
the profile server with it.

## How a script gets a login

Scripts never need a password written in them. The Python package
`netterm` asks the headless binary `netterm-cli` (`credentials`, `secret`),
which asks the running app over its **credential agent**:

- On Windows, a named pipe `\\.\pipe\netterm-agent-<id>` (the id is made
  from the data folder's path). Only your Windows account may open it (its
  access list grants your account alone), and connections from other
  computers are refused. `netterm-cli` checks that the program serving the
  pipe runs as you before asking it anything, so a program of another user
  that took the name first gets nothing.
- On Linux, a socket `<data folder>/agent.sock` that only you may open
  (mode 0600), and the app checks that whoever connects is you.

The agent only answers: nothing can be listed, stored or changed through
it. If the app is not running, or its vault is still locked,
`netterm-cli` reads Credential Manager
itself, or opens the vault after asking for the master password on the
terminal (never on its command line or from the environment); with no
terminal, as in a job the app runs, a locked vault is refused.

The login travels from the app to `netterm-cli` over the pipe and from
`netterm-cli` to the script over its output pipe. It is not written to disk
and not put in the environment; the buffers that held it are wiped. Once
the script has it, it is in the script's memory (and in Nornir's host
objects, with the Nornir plugin) like any login your own code holds.

When the app runs a script as a job, it gives it these environment
variables, none of which holds a secret: `NETTERM_WORKSPACE` (the workspace
folder), `NETTERM_CLI` (where `netterm-cli` is), `NETTERM_DATA_DIR` (so
`netterm-cli` finds this app's agent), `NETTERM_JOB_TARGETS` (a temporary
file listing the job's hosts, removed when the job ends) and, for **Fill in
from devices**, `NETTERM_FACTS_FILE`. `netterm-cli` then hands out only the
logins of the job's hosts. That limit guards against mistakes; it is not a
sandbox: a script can drop the variable, and any program running as you
can ask the agent for any login of any workspace while the app runs.

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
| A tunnel for a script                  | A script, through `netterm-cli tunnel`                                | Listens on `127.0.0.1`; reaches the device through the jump hosts |
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
  environment variable `NETTERM_UPDATE_ENDPOINT` names one (the tests use
  it). That address must be HTTPS, or plain HTTP to this computer.
- Nothing is downloaded until you click **Install and restart**, and only
  an installed copy offers that. The updater then fetches `latest.json`
  again (its user agent names the updater and its version), downloads the
  installer, checks its signature against the release key and runs it. A
  portable copy, and every copy on Linux, only shows the release's page,
  which opens in your browser.

### Tunnels for scripts

`netterm-cli tunnel` (which the Python package and its Nornir plugin start
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
- connect to Microsoft's Office servers while text is typed into a field,
  for Edge's text predictions (writing suggestions), and around forms, for
  Edge's autofill. What these send could not be seen (the connections are
  encrypted); text predictions work by sending the text being typed.

The app turns all of these off: WebView2 is started with
`--disable-background-networking`, `--no-proxy-server` and
`--disable-component-update` (and SmartScreen off), its general autofill
is off, and every field of the interface, the terminals' input included,
is marked `writingsuggestions="false"`. Each of these was seen to stop the
connections it names.

**Not solved yet:** even so, while you type (in a terminal or a field),
WebView2 now and then connects to Microsoft's Office servers (Exchange
Online addresses, port 443): in the smoke test's last full run, 8 times
across 3 of its 7 starts of the app.
These connections are made by WebView2 itself, outside the part of it
the app can watch in detail, so what they carry could not be seen, and
nothing tried so far stops them. Until it is solved, assume that what you
type in the app's window may reach Microsoft. The smoke test lists each
one it sees and fails on any other connection. WebView2 belongs to Windows and is
updated by Windows (Edge's updater), not by the app, so a later WebView2
may add something new; the smoke test watches for it (see the end of this
page).

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
  are the credential agent's pipe (your account only) and a script's
  tunnel ports (on `127.0.0.1`).
- It does not forward your SSH agent, X11 or ports to devices.
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
  get the jump host's login. Its scripts are code that runs as you. Read a
  workspace you did not write before you use it, as you would a script.
- **Tunnel ports.** While a script's tunnel runs, anyone on this computer
  can use its ports (see above).
- **A device or jump host you trusted wrongly.** The first connection
  shows the host key's fingerprint and asks; trusting a key you did not
  check accepts whoever answered.
- **TLS through a tunnel.** The app checks no TLS certificate of a device;
  a script that calls an HTTPS API through a tunnel must check it itself.
- **A forgotten master password.** There is no recovery.

## How this page is kept true

The end-to-end smoke test (`scripts/smoke.mjs`) runs the built app through
everything it can do: sessions, jump hosts, files, jobs, scripts, the
Python setup, the vault, restarts. All along it watches the connections of
the app's processes (the app, WebView2, the Python it runs and
`netterm-cli`) and the names WebView2 looks up, and fails if any of them
reaches anything but the test's own servers on this computer. How it
measures, and what it cannot see, is in the testing notes.
