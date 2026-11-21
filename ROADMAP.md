# Roadmap

What comes after v1. The order and the decisions below were settled in a design session on 2026-11-14; anything listed under "Still open" was not. The NETCONF and RESTCONF client was dropped afterwards (see "Not planned"). A second session on 2026-11-18 settled what goes into 0.5.0 (see "Next: 0.5.0"), designed the SFTP/SCP browser, and moved the file servers and NetBox sync to "Later, undated". The v1 spec lives in `docs/spec-v1.md`.

## Principles

- **Daily use drives the order.** Features are built in the order they are needed in real work, since that is what can be verified without users.
- **Small releases.** Each numbered item below ships on its own. The exception is 0.5.0, which holds the last two together (see "Next: 0.5.0").
- **Python is optional.** Everything except script jobs and host enrichment works without it.
- **Local and trusted.** The app holds some of the most sensitive passwords an organisation has, so it stays on the user's computer: no telemetry, no accounts, no cloud. The only connection it makes on its own is the update check, which can be turned off; every other connection is one the user started (a session, a Files tab or a file job, a web link, a script, a Python setup they confirmed). Nothing is added that breaks this.
- **Solid before wide.** Hardening what exists comes before showing it to anyone, and before most new features.
- **Windows first, then Linux.** The core keeps no Windows-only assumptions. macOS is not planned for now.

## Order

1. Multi-send (built)
2. Vault and master password, with named secrets (built)
3. Platform definitions, jobs and config backup (built)
4. SFTP/SCP browser (built; not released)
5. Headless binary, Python bridge and script jobs (built)
6. Host enrichment (built)
7. Linux release (built; not yet checked on a Linux desktop)
8. NetBox sync (set aside, see "Later, undated")
9. Tunnel ports for scripts (built)

## Next: 0.5.0

Settled on 2026-11-18. The two items left are built in the order 9, 4:

1. **Tunnel ports for scripts** (9): built on 2026-11-18 (spec #91, tickets #92 and #93).
2. **SFTP/SCP browser** (4): the most use day to day. Designed (spec #95), prototyped against the lab (#96) and built on 2026-11-19 to 2026-11-21 (#101 to #108). Its sweep (#108) walked every user story in the built app and on the lab's devices; not released yet.

These are the last features before the app goes into daily use. Nothing was to be released in between; on 2026-11-19 0.4.1 was released all the same, with the tunnel ports, the mouse fixes (#94) and what a renamed host keeps (#99). Each feature ends with a sweep of its spec's user stories; then comes a second pass over Hardening, reordered by what has been found by then; then one release, 0.5.0, still an unsigned pre-release. That release is installed and run on Windows before it is published (the check owed since 0.3.0), and daily use starts with it.

Alongside, outside the two features: the end-to-end test clicks with the mouse, as a user does (#94). In 0.3.0 and 0.4.0 the host-key question could not be answered with the mouse, because the terminal's drawing layers lay over it, and the test passed all the same, since its clicks reach a control whatever covers it. The fix is in 0.4.1.

## Status

As of 2026-11-21, every item but 8 is built. Items 1, 2, 3, 5, 6 and 7 were released in 0.2.0 and item 9 in 0.4.1. Item 4 is built and not released: it is what 0.5.0 adds. Item 8 was set aside. Two releases since came from Hardening, not from the list above: 0.3.0 (the redesigned interface) and 0.4.0 (the app's own window frame, and terminals in windows of their own). 0.4.1 holds item 9 and fixes. Every release so far is an unsigned pre-release. What was built but not verified:

- **The release files themselves:** the installer, the portable zip and the Linux packages of 0.3.0 and 0.4.0 were built and checksummed, not installed and run as part of the release. The end-to-end test runs the same code built without the installer.
- **Linux desktop:** the packages build and the core, the headless binary and the Python bridge pass their tests under WSL. The app itself has not been run on a real Linux desktop; the check list is in `docs/linux.md`.
- **NX-OS:** its built-in platform definition has not been run against a device (no image in the lab). The other five are verified in the containerlab suite.
- **Privilege password:** no lab account asks for an enable password, so that question has only been exercised against the test device.
- **Files:** checked on the five lab devices, which are virtual. Not verified: NX-OS, the name of flash on physical Catalyst switches, speeds on real hardware, a jump host that needs a typed login, a real Explorer drag and the real file dialog (the end-to-end test plays both), Linux. The list is in [docs/files.md](docs/files.md).

## Hardening

What has to be true before anyone but the developer uses the app. It runs alongside the remaining features and comes first where the two compete. So far the app has had one user and was tested mostly against test servers and one lab.

The parts below are not in an order yet: they are ordered in the pass that follows the features of 0.5.0. Three have tickets: the Linux desktop check (#58), the end-to-end test's clicks (#94) and what a renamed host loses (#99). The rest is still to be ticketed.

**The first five minutes**

- Someone who has never seen the app installs it, brings in their hosts (CSV or Nornir import, or by hand) and is connected to a device, without reading documentation.
- Walk that path from a clean Windows profile and fix every place it stalls: the empty first window, creating or opening a workspace, the first credential profile, the first host-key question, the first failed login.
- Error messages on that path say what to do next.

**Redesign** (built)

- Done on 2026-11-15: the spec is issue #61 and its tickets #62 to #79 in the project's tracker, all closed, and released in 0.3.0. [docs/interface.md](docs/interface.md) describes the result and where each feature of 0.2.0 now lives.
- In short: an activity rail with one foldable side view replaces the toolbar and the permanent side columns; jobs, config history, preferences and the imports open as tabs beside the terminals; only short confirmations stay dialogs; job runs and config history can open as real windows. Behaviour does not change.
- Found in use after 0.3.0: the system title bar wasted a line and did not match, so the app now draws its own window frame (#80, built). The cosmetic fixes made after 0.3.0 and the window frame are in 0.4.0.
- Also found in use: terminals could not leave the main window, which engineers with several monitors need when troubleshooting across devices (spec #81, closed). The risky part, handing a live session from one window to another without losing output, was proven by a prototype (#82) and its mechanism is in the core with tests. Built on 2026-11-15 (#83 to #90) and released in 0.4.0: a session moves to a window of its own and back, session windows hold tabs and split panes, and safety markers, questions, multi-send and the closing rules follow. Checked with twenty sessions over three windows on two monitors. A session window whose page crashes hands its sessions to the main window (#89). A new session window is placed by physical position, so it opens beside the main window on monitors with different scaling too (#90, checked at 100% and 150%). Still open: a session window whose page hangs without crashing, or the web engine dying as a whole, leaves sessions stuck until the app is closed; the crash handling is untried on a Linux desktop.
- Small limits of the new window, known and left for now: Windows 11's snap-layouts menu on the Maximize button is not offered (dragging to an edge and the Win+arrow keys still snap); a window cannot be moved while a window-wide dialog is open. In a narrow top-right pane the pane's own tools (join, split, close pane) used to be cut off and could not be clicked; since #100 they fold into a "…" menu (in 0.4.1).
- Found on 2026-11-19, in 0.4.0 and on main alike, not ticketed: when the top-right pane's bar gets narrower than about 175 px, the window's own Minimize, Maximize and Close start to leave the window. With the side view open that is a main window under about 560 px wide, or under about 685 px with two panes side by side; the window can be made as narrow as 480 px. Folding the side view brings them back.

**Edge testing**

Deliberately hostile use, each finding becoming a test where it can:

- **Sessions:** hours-long sessions, very large and very fast output, a link that drops or stalls mid-command, the computer sleeping and waking, a device that closes without warning, twenty or more tabs, odd prompts and banners, wide and combining characters, pasting very large text.
- **Devices:** old ones needing legacy crypto, slow consoles, a real serial adapter (break, unplugging), jump hosts that drop or go silent.
- **Inventory:** thousands of hosts, malformed and half-written YAML, a `git pull` or an editor changing files while the app has them open, names with unusual characters, network drives.
- **Credentials:** a damaged or half-written vault file, power loss during a write, two copies of the app at once, a locked vault met by every feature that needs a login.
- **Jobs and scripts:** hundreds of hosts, hosts failing in every way at once, cancelling at every stage, a script that hangs, floods output or leaves processes behind.
- **Windows itself:** display scaling and several monitors, a locked-down corporate laptop (no admin rights, restricted PowerShell, a strict firewall), antivirus holding files open.

Done so far, on 2026-11-15 for the session windows (the record is in the window notes): twenty SSH sessions over three windows with one flooding for a minute, and two monitors at the same scale and at 100% beside 150%. A monitor left of or above the primary is untried. Everything else in the list is still to do.

**Checks still owed from the work so far**

- A review of the 0.2.0 work (#34 to #57) against its spec. The script tunnels (#60) were reviewed after they were built, and the redesign and the session windows each ended with a sweep of their spec's user stories (#79, #88); the rest of 0.2.0 was not.
- A release installed and run: the Windows installer and the portable zip. This is done with 0.5.0, before it is published. The Linux packages are part of the Linux desktop check (#58).
- The items listed under Status as built but not verified.

**Trust, written down**

- One plain document: where passwords and keys are kept, what is encrypted and what is not (logs, job results and config backups are plain files), and every connection the app can make and why.
- A test that the app makes no connection on its own with the update check off (the smoke test already checks this at start; extend it over a whole run).

**Going public**

- The source moves to GitHub once the above gives enough confidence, not before.
- Then, in this order: the updater signing key, a public address for releases, code signing and a `winget` listing.

## 1. Multi-send

Type into several open terminals at once.

- Every keystroke is sent live to a group of tabs, so tab completion and `?` help work.
- Tabs are added to the group explicitly, and each one shows a clear marker.
- Adding a production tab asks for confirmation.
- Pastes still go through each terminal's own paste safety.
- The group is forgotten when the app closes.

## 2. Vault and master password

An encrypted store for credentials, for users who want more than the system keychain.

- Opt-in. The keychain stays the default, so the app works with no setup.
- The vault is the app's own encrypted file, with the key derived from the master password. It is another `CredentialStore` implementation.
- The master password is asked once at launch. The vault stays unlocked until the app exits: no inactivity timer, no reaction to the computer locking.
- The file lives in the per-user data folder, never in the workspace.
- A forgotten master password has no recovery; profiles are entered again. The dialog that sets the password says so.
- Turning the vault on moves the keychain's profiles into it; turning it off moves them back.
- **Named secrets:** the store (vault or keychain) also holds name → value secrets per workspace, for scripts and integrations, such as an API token a script needs.
- Designed with the agent pipe (item 5) and Linux in mind, so neither needs a second pass.

## 3. Platform definitions, jobs and config backup

Act on many devices at once, with config backup as the proving use case.

**Platform definitions**

- A definition has seven values: prompt pattern, disable-paging command, privilege command, privilege password prompt, show-config command, a pattern for lines to ignore when comparing configs, and the file folder (where files belong on the device, such as `flash:` or `/mnt/flash`; #102). All but the prompt are optional.
- Stored in `_platforms.yaml` at the workspace root and shared through git, but created and edited only in the app.
- A definition is matched to a host by its `platform` field, ignoring case. The field becomes a dropdown of defined platforms that still accepts typed text, and it can be set on a folder so a folder of identical devices needs one assignment.
- Built in: SR Linux, EOS, FRR, IOS, NX-OS and Junos, each verified in the containerlab suite before it ships (NX-OS is still to verify).
- The engine sends the credential profile's enable secret at the privilege prompt.

**Jobs**

- Native engine in the core. A job opens its own headless sessions and never touches open terminals, so it reuses jump hosts, host-key checking, legacy crypto and agent logins exactly as the terminal does.
- Pick hosts or a folder, pick a saved snippet, run it, and get per-device output in a results view that can be searched, diffed and saved.
- Ad hoc only at first, with "run again" on a past result.
- Snippet variables fill from each host. Missing ones are asked once and apply to all hosts.
- A job never asks mid-run. A pre-flight list shows the hosts it will skip (no stored profile, not SSH) and asks once if any target is production.
- During the run, an untrusted or changed host key or an interactive login question fails that host with the reason; the rest continue.
- 10 hosts in parallel by default, adjustable per run.
- Results are plain files outside the workspace, beside the session logs and keyed by workspace id. They are kept until deleted, with "delete" and "save to…" on each.

**Config backup**

- A job that runs the platform's show-config command and saves the result per device.
- **Backup on login:** an inherited setting, like `logging`. When a session opens, the config is fetched over a second channel on the same SSH connection, so there is no second login and nothing appears in the terminal. If the device refuses a second channel, a second headless connection is used, for stored-profile logins only. Not offered for telnet or serial.
- A new version is saved only when the config differs from the last one, ignoring the platform's ignore-lines pattern. The saved file keeps those lines.
- History opens from the host side panel, with a diff between any two versions.
- Nothing is pruned automatically.
- Backups are plain files outside the workspace. The docs say plainly that they are as sensitive as the configs themselves.
- Checked in the lab: SR Linux, cEOS, FRR and cJunos accept the second channel; IOS-XE refuses it, so its backup on login uses the second connection.

## 4. SFTP/SCP browser

Move images and configs to and from devices without a separate tool.

- SFTP and SCP file browser for a host, opened from the side panel.
- Push a file to a device from the inventory.
- SFTP uses `russh-sftp`; SCP has no library support and runs over a remote command.
- Designed on 2026-11-18 (spec #95). The prototype against the lab devices (#96) reported the same day, and the build tickets were written from it on 2026-11-19 (#101 to #108). **Built** by 2026-11-21; not released yet. [docs/files.md](docs/files.md) is the user's page.
  - A Files tab per host, beside the terminals. It logs in with the host's credential profile through its jump hosts, on a connection of its own, so the terminal is never slowed. A login that must be typed and an unknown host key are asked in the tab.
  - Devices with SFTP get a browser: list, upload, download, rename, delete, new folder. Devices with SCP only (IOS, IOS-XE) get transfer by path; the app falls back by itself.
  - The local side is the operating system: a file dialog, files dropped from Explorer, a folder to download into.
  - Transfers show progress and speed and can be cancelled; one at a time per host. Sizes are checked, and the uploaded file's MD5 and SHA-256 are shown for comparing with the device's own verify command. No resume.
  - On a production host, uploading, deleting and renaming ask first; overwriting asks wherever the device can say the file is there.
  - A platform definition gains a seventh value, the folder where files belong on that platform.
  - "Send a file to many hosts" is a job step, for staging an image on a folder of devices: three in parallel by default, a host that already has the file counts as done, and the kept run records what was sent where.
- Left out: editing in place, whole folders, a two-pane view, using the open terminal's connection, fetching a file from many hosts, file commands in the headless binary.
- What the prototype found (the detail is on #96 and in `docs/containerlab.md`):
  - The SFTP library works with the pinned SSH library and, once tuned, moves a 500 MB file as fast as OpenSSH over the same path.
  - SR Linux, cEOS and FRR offer SFTP as they come; cJunos offers it once its SFTP server is turned on, and SCP before that; IOS-XE offers SCP only, and only once `ip scp server enable` is set.
  - IOS-XE closes the connection after each SCP transfer and has five lines in all, so on a device with SCP only the app connects per file and holds no connection in between.
- Decided from it: the app runs no commands on a device to make up for what SCP lacks. So on a device with SCP only there is no listing, an existing file is replaced without a question (the form says so), a wrong folder fails only once the whole file has been sent, and the file job cannot tell that the file is already there: such a host fails unless "overwrite" is ticked.
- The closing sweep (#108, 2026-11-21) walked the spec's user stories in the built app, against the example server and the lab's devices through its jump host, and added a files check to the containerlab suite (`lab_files`: SFTP on SR Linux, cEOS, FRR and cJunos; SCP on the CSR 1000v).
  - On cEOS a 400 MB upload ran at about 100 MB/s while a terminal on the same device answered as quickly as before it. The same file went as a job to cEOS, cJunos and the CSR (about 2.7 MB/s over SCP there), and each device's own checksum matched the MD5 the run shows.
  - It found one fault, fixed there: IOS-XE ends the whole connection when it closes the SFTP channel it has no server for, and a file job took that for a lost connection instead of falling back to SCP. The app now goes by the channel the device closed in order.
  - One difference from the design, kept: on a device with SCP only a login typed in the tab is kept while the tab is open and used for each file, not asked again per file. A login that asks a question each time (a one-time code) still fails each transfer, since nobody is asked.

## 5. Headless binary, Python bridge and script jobs

Let existing automation use the inventory.

- A headless command-line binary, built from the core and shipped beside the app, serves the resolved inventory and credentials. Inheritance and jump chains have one implementation.
- A small pip package exposes the inventory as a Nornir inventory, so existing Netmiko, scrapli, Nornir and NAPALM scripts run unchanged, from the app or from a plain shell.
- The package finds the binary through `NETTERM_CLI` (the app sets it for scripts it launches), else the default install location, else `PATH`.
- Python 3.10 and newer.
- **Environment:** the app offers to create its own venv in the data folder and install the package plus chosen libraries, after an explicit confirmation listing what will be installed. The user can point it at their own interpreter instead. Nothing is installed without the confirmation, and the feature is off until one of the two is set.
- **Credentials:** handed over a local pipe on request, never written to disk and never put in environment variables. With the vault on, the running app acts as an agent, as `ssh-agent` does; with no app running, the binary asks for the master password on the terminal. Unattended scheduled runs against a vault are deliberately not possible.
- Scripts read named secrets the same way, by name.
- **Jump hosts:** scripts reach hosts behind jump hosts through loopback tunnels the binary opens with the app's own jump chains (stored-profile or agent logins, trusted host keys, legacy crypto, timeouts). One tunnel process serves a script's hosts, shares a jump connection between hosts behind the same bastion and ends with the script. Libraries check the device's key against the keys the app trusts; a device the app has never trusted is refused until it is connected to once. Telnet hosts behind jump hosts stay unreachable for scripts.
- **Script jobs:** "run this script against the selected hosts" is a second job type, with output streamed into the results view. Config-mode pushes that need Netmiko's or scrapli's platform handling belong here.
- Saved jobs (name, snippet or script, target folder or saved filter) arrive with this release.
- Licensing: netterm is GPLv3; these libraries are MIT or Apache-2.0, installed by the user and run in a separate process.

## 6. Host enrichment

- A bundled NAPALM script, run through the bridge, fills in vendor, model, serial number and OS for hosts already in the inventory.
- It fills empty fields and never overwrites typed values without asking.
- Hosts behind jump hosts are asked through the bridge's tunnels.
- It assumes the engineer already uses Python automation; everyone else has manual entry and the CSV and Nornir imports.
- No subnet scanning.

## 7. Linux release

- After the bridge, so the vault, agent pipe and interpreter handling are ported once.
- The vault covers systems with no keychain service.

## 8. NetBox sync

Set aside on 2026-11-18; see "Later, undated".

## 9. Tunnel ports for scripts

Let scripts reach more than a host's SSH port through the app's jump hosts.

- Today a script's tunnel reaches only the host's SSH port. A script can name other ports of the host (830 for NETCONF, 443 for RESTCONF or eAPI), and the tunnel carries them through the same jump chain.
- This is how model-driven access is served: the engineer's own Python libraries (ncclient, scrapli-netconf, requests) with the app's inventory, credentials and jump hosts. The app has no client of its own for it, and ships no scripts that push changes or discover interfaces.
- A small change to the tunnels that exist; no new subsystem.
- Designed on 2026-11-18 (spec #91):
  - A script names the ports when it opens the tunnels: an argument in Python, an option of the Nornir plugin, a flag of the headless binary. Nothing is added to the inventory or the interface.
  - One helper tells a script where to connect for a port of a host, whether the host is behind a jump host or not, so the same script runs in both cases.
  - For NETCONF, which runs over SSH, the library checks the device against the keys the app trusts for it.
  - For HTTPS the app does not check the device's certificate: the path is protected up to the last jump host, and the script decides how to check beyond it. The docs say so.
  - Any TCP port can be named. Fill in from devices does not change: it already works over the SSH port.
- **Built** on 2026-11-18 (#92: the core and `netterm-cli tunnel --port`; #93: `netterm.tunnels(..., ports=)`, `netterm.endpoint`, the Nornir plugin's `tunnel_ports` and `netterm.ports`, host-key options for ncclient and scrapli-netconf, the README). Checked in the lab through its jump host (`lab_tunnel_ports`): ncclient to cJunos Evolved 25.4 on 830 with the device key checked and another key refused, and an HTTPS eAPI call to cEOS 4.36 on 443. Not released yet.

## Later, undated

- NetBox sync (was item 8; set aside on 2026-11-18). Nobody on the project uses NetBox and the lab cannot stand in for a real one, so it could not be verified. If it comes back, it is as something of its own beside the app, closer to a plugin than a built-in feature: read-only and one-way, hosts written as ordinary host files into one folder, never in the background. A first design is kept in the closed spec #97; nothing it says about NetBox itself was ever checked against a running instance.
- Temporary TFTP, FTP and SFTP servers that serve a chosen folder (was item 10; moved here on 2026-11-18). They would be the only part of the app that listens on the network. The SFTP/SCP browser pushes files to devices, which covers every device that has a file server of its own; what is left is old devices that can only fetch over TFTP, and there is none to test with. Still to design if it comes back: authentication, transfer logging, low ports, the Windows firewall prompt, locked-down corporate laptops.
- Dragging a tab out of the window to give it a window of its own, and dragging tabs between windows (left out of #81: the menu and the shortcut came first)
- Remembering session windows and their places across restarts (left out of #81, like the tab layout)
- Expect-style scripting of live terminal sessions
- Port forwarding and SOCKS proxy
- Quick tools: ping, traceroute, subnet calculator, MAC vendor lookup
- A clickable topology map generated from LLDP/CDP neighbour data
- macOS release

## Not planned

- Import from PuTTY, SecureCRT or MobaXterm
- Subnet scanning or discovery of unknown devices
- A manual diagram editor
- Telemetry, crash reporting to third parties, AI features
- Hosted sync, accounts or any cloud service
- Embedded web consoles or stored web logins
- An embedded Python interpreter
- Locking the app on inactivity or when the computer locks
- Encrypting session logs, job results or config backups
- A NETCONF and RESTCONF client (dropped on 2026-11-14). netterm is a CLI tool. NETCONF would have to be written from scratch in the core (no mature Rust crate), with an editor and saved collections on top, to compete with the Python libraries its users already have; RESTCONF adds certificate trust for self-signed devices; and ready-made data pulls would need upkeep per platform and version that belongs in the user's own scripts. Scripts get there through tunnel ports (item 9). Experimental tools like this may be looked at again, but only once the tool is good at what it already does.

## Still open

- **Edge testing:** there are no real devices or adapters to test with (settled on 2026-11-18): everything is checked against the containerlab images. A real serial adapter, old hardware and a locked-down corporate laptop stay untested until someone has one.

- **Linux:** the check on a real desktop (Ubuntu and Fedora, X11 and Wayland), before Linux is claimed as supported.
- **Script tunnels:** job pre-flight does not know host keys, so a host whose device or jump-host key is not trusted yet is refused only when the job runs.
- **Tunnel ports:** the app checks no TLS certificate (eAPI, RESTCONF, NX-API through a tunnel are only as safe as the script's own check beyond the last jump host); whether certificates should ever be learned or pinned like host keys is undecided. ncclient fails to negotiate with a device for which the app trusts only an RSA key and that offers RSA only as `rsa-sha2-*` (an ncclient limit). scrapli-netconf's `system` transport, gNMI, RESTCONF and NX-API were not tried; the ports were tried on Windows only.
- **SFTP/SCP browser:** the lab is virtual, so speeds on real hardware are unknown (the lab's CSR is held to 1 Mbit/s). Not verified: NX-OS (no image), the name of flash on physical Catalyst switches, a jump host that needs a typed login. Whether the file job should be able to ask an IOS device if the file is already there, which needs a command per platform, is left until it is missed. A job takes a folder, a saved filter or one host: the tree has no selection of several hosts, for the file job as for every job. On a device with SCP only, a login that asks a question each time (a one-time code) or a host key trusted once is asked over the tab for each file (#109), as the design had it; no lab device asks a question at login, so this was verified against the test server only.