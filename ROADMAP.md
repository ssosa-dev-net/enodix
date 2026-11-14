# Roadmap

What comes after v1. The order and the decisions below were settled in a design session on 2026-11-14; anything listed under "Still open" was not. NETCONF/RESTCONF, the SFTP/SCP browser, the file servers and NetBox sync each get their own design session before work starts. The v1 spec lives in `docs/spec-v1.md`.

## Principles

- **Daily use drives the order.** Features are built in the order they are needed in real work, since that is what can be verified without users.
- **Small releases.** Each numbered item below ships on its own.
- **Python is optional.** Everything except script jobs and host enrichment works without it.
- **Windows first, then Linux.** The core keeps no Windows-only assumptions. macOS is not planned for now.

## Order

1. Multi-send
2. Vault and master password, with named secrets
3. Platform definitions, jobs and config backup
4. SFTP/SCP browser
5. Headless binary, Python bridge and script jobs
6. Host enrichment
7. Linux release
8. NetBox sync
9. NETCONF and RESTCONF client
10. Temporary file servers

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
- Built in: SR Linux, EOS, FRR, IOS, NX-OS and Junos, each verified in the containerlab suite before it ships.
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
- Jobs treat CLI snippets and later request types the same way, so NETCONF/RESTCONF requests plug in without a second mechanism.

**Config backup**

- A job that runs the platform's show-config command and saves the result per device.
- **Backup on login:** an inherited setting, like `logging`. When a session opens, the config is fetched over a second channel on the same SSH connection, so there is no second login and nothing appears in the terminal. If the device refuses a second channel, a second headless connection is used, for stored-profile logins only. Not offered for telnet or serial.
- A new version is saved only when the config differs from the last one, ignoring the platform's ignore-lines pattern. The saved file keeps those lines.
- History opens from the host side panel, with a diff between any two versions.
- Nothing is pruned automatically.
- Backups are plain files outside the workspace. The docs say plainly that they are as sensitive as the configs themselves.

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
- **Script jobs:** "run this script against the selected hosts" is a second job type, with output streamed into the results view. Config-mode pushes that need Netmiko's or scrapli's platform handling belong here.
- Saved jobs (name, snippet or script, target folder or saved filter) arrive with this release.
- Licensing: netterm is GPLv3; these libraries are MIT or Apache-2.0, installed by the user and run in a separate process.

## 6. Host enrichment

- A bundled NAPALM script, run through the bridge, fills in vendor, model, serial number and OS for hosts already in the inventory.
- It fills empty fields and never overwrites typed values without asking.
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

## 9. NETCONF and RESTCONF client

**Goal:** one tool for CLI and model-driven access, with a Postman-like experience that knows the device.

- Saved request collections stored as YAML in the workspace, shareable through git.
- Variables filled from the selected host and credentials from its profile.
- Raw XML and JSON editor with a formatted response view.
- Device capabilities view for NETCONF.
- Requests available as job steps across many hosts.
- Later: a YANG model browser.
- NETCONF is implemented in the core on the SSH subsystem channel, with both framing styles (1.0 end-of-message and 1.1 chunked), so it reuses jump hosts, host-key checking and the legacy-crypto switch. No mature Rust crate exists for this.
- Opens from the host side panel alongside SSH, telnet, serial and web links.
- Gets its own design session.

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
- `winget` listing and code signing at public launch

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

## Still open

- **Backup on login:** which platforms accept a second channel on one SSH connection. To be checked in the lab.
- **SFTP/SCP browser:** the whole design.
- **NetBox:** how its hierarchy (region, site, location, rack) maps onto the folder tree, and where local overrides are kept.
- **NETCONF/RESTCONF:** where collections sit in the workspace and whether they inherit like snippets and web links; how RESTCONF device certificates are trusted, given the self-signed ones common on network devices.
- **File servers:** authentication, transfer logging, low ports (TFTP 69, FTP 21), the Windows firewall prompt, and behaviour on locked-down corporate laptops.
