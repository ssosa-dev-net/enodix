# Roadmap

What comes after v1. The order and the decisions below were settled in a design session on 2026-11-14; anything listed under "Still open" was not. The NETCONF and RESTCONF client was dropped afterwards (see "Not planned"). The SFTP/SCP browser, the file servers and NetBox sync each get their own design session before work starts. The v1 spec lives in `docs/spec-v1.md`.

## Principles

- **Daily use drives the order.** Features are built in the order they are needed in real work, since that is what can be verified without users.
- **Small releases.** Each numbered item below ships on its own.
- **Python is optional.** Everything except script jobs and host enrichment works without it.
- **Local and trusted.** The app holds some of the most sensitive passwords an organisation has, so it stays on the user's computer: no telemetry, no accounts, no cloud. The only connection it makes on its own is the update check, which can be turned off; every other connection is one the user started (a session, a web link, a script, a Python setup they confirmed). Nothing is added that breaks this.
- **Solid before wide.** Hardening what exists comes before showing it to anyone, and before most new features.
- **Windows first, then Linux.** The core keeps no Windows-only assumptions. macOS is not planned for now.

## Order

1. Multi-send (built)
2. Vault and master password, with named secrets (built)
3. Platform definitions, jobs and config backup (built)
4. SFTP/SCP browser
5. Headless binary, Python bridge and script jobs (built)
6. Host enrichment (built)
7. Linux release (built; not yet checked on a Linux desktop)
8. NetBox sync
9. Tunnel ports for scripts
10. Temporary file servers

## Status

As of 2026-11-14, items 1, 2, 3, 5, 6 and 7 are built and released in 0.2.0, an unsigned pre-release like 0.1.x. Item 4 was skipped for now, as it still needs its design session. What was built but not verified:

- **Linux desktop:** the packages build and the core, the headless binary and the Python bridge pass their tests under WSL. The app itself has not been run on a real Linux desktop; the check list is in `docs/linux.md`.
- **NX-OS:** its built-in platform definition has not been run against a device (no image in the lab). The other five are verified in the containerlab suite.
- **Privilege password:** no lab account asks for an enable password, so that question has only been exercised against the test device.
- **Host enrichment through tunnels:** checked against the in-process test servers; the lab run through the lab's jump host is still to do.

## Hardening

What has to be true before anyone but the developer uses the app. It runs alongside the remaining features and comes first where the two compete. So far the app has had one user and was tested mostly against test servers and one lab.

**The first five minutes**

- Someone who has never seen the app installs it, brings in their hosts (CSV or Nornir import, or by hand) and is connected to a device, without reading documentation.
- Walk that path from a clean Windows profile and fix every place it stalls: the empty first window, creating or opening a workspace, the first credential profile, the first host-key question, the first failed login.
- Error messages on that path say what to do next.

**Redesign**

- Specified and ticketed on 2026-11-15: the spec is issue #61 and its tickets are #62 to #79 in the project's tracker. They hold the design; this roadmap no longer does.
- In short: an activity rail with one foldable side view replaces the toolbar and the permanent side columns; jobs, config history, preferences and the imports open as tabs beside the terminals; only short confirmations stay dialogs; job runs and config history can open as real windows. Behaviour does not change.

**Edge testing**

Deliberately hostile use, each finding becoming a test where it can:

- **Sessions:** hours-long sessions, very large and very fast output, a link that drops or stalls mid-command, the computer sleeping and waking, a device that closes without warning, twenty or more tabs, odd prompts and banners, wide and combining characters, pasting very large text.
- **Devices:** old ones needing legacy crypto, slow consoles, a real serial adapter (break, unplugging), jump hosts that drop or go silent.
- **Inventory:** thousands of hosts, malformed and half-written YAML, a `git pull` or an editor changing files while the app has them open, names with unusual characters, network drives.
- **Credentials:** a damaged or half-written vault file, power loss during a write, two copies of the app at once, a locked vault met by every feature that needs a login.
- **Jobs and scripts:** hundreds of hosts, hosts failing in every way at once, cancelling at every stage, a script that hangs, floods output or leaves processes behind.
- **Windows itself:** display scaling and several monitors, a locked-down corporate laptop (no admin rights, restricted PowerShell, a strict firewall), antivirus holding files open.

**Checks still owed from the work so far**

- A review of everything built after 0.1.2 against its spec; only the script tunnels were reviewed after they were built.
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
- **Named secrets:** the store (vault or keychain) also holds name → value secrets per workspace, for scripts and integrations. The NetBox API token is the first.
- Designed with the agent pipe (item 5) and Linux in mind, so neither needs a second pass.

## 3. Platform definitions, jobs and config backup

Act on many devices at once, with config backup as the proving use case.

**Platform definitions**

- A definition has six values: prompt pattern, disable-paging command, privilege command, privilege password prompt, show-config command, and a pattern for lines to ignore when comparing configs. All but the prompt are optional.
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
- Gets its own design session.

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

- Read-only and one-way; NetBox stays the source of truth.
- Synced hosts are ordinary host files, committed to the workspace, confined to one designated folder subtree and marked with a `netbox` key. One sync is one reviewable commit.
- Sync is a deliberate action, never a background one.
- Local-only overrides (credential profile, notes) survive a refresh.
- The API token is a named secret.
- Waits for a lab NetBox instance to test against, and gets its own design session.

## 9. Tunnel ports for scripts

Let scripts reach more than a host's SSH port through the app's jump hosts.

- Today a script's tunnel reaches only the host's SSH port. A script can name other ports of the host (830 for NETCONF, 443 for RESTCONF or eAPI), and the tunnel carries them through the same jump chain.
- This is how model-driven access is served: the engineer's own Python libraries (ncclient, scrapli-netconf, requests) with the app's inventory, credentials and jump hosts. The app has no client of its own for it.
- A small change to the tunnels that exist; no new subsystem.
- Open: how a script names the ports, and what a library is given to check an HTTPS certificate by, as there is no host key to hand over.

## 10. Temporary file servers

- One-click temporary TFTP, FTP and SFTP servers that serve a chosen folder on a chosen network interface and stop when closed.
- They are the only part of the app that listens on the network. They must be off by default, clearly visible while running, and bound to one interface.
- Gets its own design session.

## Later, undated

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

- **Edge testing:** which real devices and adapters are available beyond the containerlab images.

- **Linux:** the check on a real desktop (Ubuntu and Fedora, X11 and Wayland), before Linux is claimed as supported.
- **Script tunnels:** job pre-flight does not know host keys, so a host whose device or jump-host key is not trusted yet is refused only when the job runs.
- **SFTP/SCP browser:** the whole design.
- **NetBox:** how its hierarchy (region, site, location, rack) maps onto the folder tree, and where local overrides are kept.
- **Tunnel ports:** how a script names the ports, and certificate checking for HTTPS through a tunnel.
- **File servers:** authentication, transfer logging, low ports (TFTP 69, FTP 21), the Windows firewall prompt, and behaviour on locked-down corporate laptops.
