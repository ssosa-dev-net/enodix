# netterm v1 spec

`netterm` is a working codename. The user-visible name is not chosen yet.

## Problem Statement

Network engineers spend their day connecting to routers, switches, firewalls and console ports, and the tools for it are poor. PuTTY is free but old and has no real inventory. SecureCRT is capable but expensive for what it is. The modern free terminals (Tabby, WindTerm, the Termius free tier, MobaXterm Home) are general-purpose: they keep a flat or shallow list of sessions and know nothing about the network those sessions belong to.

As a result an engineer keeps device knowledge (site, room, rack, role, management address, which controller manages it) in spreadsheets or in their head, and uses one tool for the CLI, another for the serial console, a browser bookmark folder for web consoles, and something else again for API calls. Consultants who move between customers have the extra problem of keeping each customer's devices and credentials apart.

## Solution

A free, open-source desktop app: the terminal that knows your network. The engineer organises devices in a hierarchical inventory that mirrors the real network (for example Region → Site → Building → Room → Rack → Device), records real device information on each host, and reaches any device in one action over SSH, telnet or a serial cable. Connection settings are set once on a folder and inherited by everything under it. Web consoles and controllers are one click away from the same host entry. Each customer or network is a separate workspace stored as plain files that can be shared through git, with credentials kept out of those files in the operating system's keychain.

v1 covers the terminal and the inventory. Later phases add jobs across many devices, NetBox sync, Python scripting with device discovery, a NETCONF/RESTCONF client and file transfer.

## User Stories

### Workspaces

1. As a consultant, I want a separate workspace per customer, so that one customer's devices and credentials never mix with another's.
2. As a consultant, I want to switch workspace quickly from inside the app, so that moving between engagements takes seconds.
3. As a network engineer, I want a workspace to be an ordinary folder of readable files, so that I can back it up, inspect it and move it without the app.
4. As a team member, I want to keep a workspace in git, so that my team shares one inventory and sees who changed what.
5. As a team member, I want workspace files to diff and merge cleanly, so that two people editing different devices do not conflict.
6. As a consultant, I want to hand a customer their workspace folder at the end of an engagement, so that they keep the inventory I built.
7. As a security-conscious engineer, I want workspace files to contain no passwords or keys, so that sharing the folder never leaks credentials.
8. As a new user, I want to create an empty workspace or open an existing folder, so that I can start from nothing or from a colleague's repository.

### Inventory structure

9. As a network engineer, I want a tree of folders of any depth, so that the inventory can mirror my real hierarchy.
10. As a network engineer, I want to create, rename, move and delete folders and hosts from the tree, so that I can maintain the inventory without editing files.
11. As a network engineer, I want to drag a host or folder to another place in the tree, so that reorganising is quick.
12. As a network engineer, I want to tag hosts, so that I can group them in ways the tree does not express.
13. As a network engineer, I want saved filters (for example "all firewalls" or "vendor is Arista"), so that I can view the inventory by role or vendor without a second tree.
14. As a network engineer, I want changes made to workspace files outside the app (for example by a git pull) to appear in the app, so that the tree is never stale.

### Host information

15. As a network engineer, I want built-in fields on each host for management address, vendor, platform, model, serial number, site, building, room, rack, rack unit, role and description, so that the essentials are captured consistently.
16. As a network engineer, I want to add my own key/value fields to a host, so that I can record anything the built-in fields miss.
17. As a network engineer, I want a free-text description on hosts and folders, so that I can leave notes for myself and colleagues.
18. As a network engineer, I want to assign each host a role such as core, distribution, access, edge router, firewall, load balancer, wireless controller, access point, console server or server, so that I know where a device sits in the topology.
19. As a network engineer, I want to add my own roles to a workspace, so that the list fits my network.
20. As a network engineer, I want each role to have an icon and colour shown in the tree and on session tabs, so that I always see what kind of device I am typing into.
21. As a network engineer, I want a side panel showing the selected host's information, so that I can read rack and site details while connected.
22. As a network engineer, I want to copy a field value such as the management address with one click, so that I can paste it elsewhere.

### Inherited settings

23. As a network engineer, I want to set a credential profile on a folder, so that every device under it uses that login without my repeating it.
24. As a network engineer, I want to set a jump host on a folder, so that every device at a site is reached through that site's bastion.
25. As a network engineer, I want to set protocol, port, legacy-crypto, logging and environment on a folder, so that a whole site or device class is configured once.
26. As a network engineer, I want to override any inherited setting on a subfolder or host, so that exceptions are easy.
27. As a network engineer, I want to see where each effective setting on a host comes from, so that I can tell why a device connects the way it does.

### Search and navigation

28. As a network engineer, I want a command palette opened with Ctrl+K that finds any host by name, address, tag or field value, so that I can jump to a device without browsing the tree.
29. As a network engineer, I want search results to appear instantly even with thousands of hosts, so that the palette is faster than the tree.
30. As a network engineer, I want to connect straight from a search result, so that finding and connecting is one flow.

### Credentials

31. As a network engineer, I want named credential profiles holding a username with a password or SSH key and an optional enable secret, so that I define a login once and reuse it.
32. As a network engineer, I want secrets stored in the operating system's keychain, so that they are protected by my OS login and not by a file the app invented.
33. As a network engineer, I want to use keys held in my SSH agent (OpenSSH agent or Pageant), so that private keys never leave the agent.
34. As a team member, I want a shared workspace to refer to a profile by name only, so that each colleague supplies their own secret for that name.
35. As a team member, I want a clear prompt when a workspace refers to a profile I have not set up, so that I know what to create.
36. As a network engineer, I want to be prompted for a password when a host has no profile, so that I can connect ad hoc.

### SSH

37. As a network engineer, I want to double-click a host and get an SSH prompt in a new tab, so that reaching a device is one action.
38. As a network engineer, I want password, public-key, agent and keyboard-interactive authentication, so that I can log in to devices however they are configured.
39. As a network engineer, I want the app to remember a device's host key on first connection and warn me loudly if it changes, so that I am protected from interception.
40. As a network engineer, I want to reach a device through one or more jump hosts, so that I can work on networks only reachable through a bastion.
41. As a network engineer, I want a jump host to be another host in my inventory, so that it uses the same credentials and settings model.
42. As a network engineer, I want a per-host or per-folder legacy-crypto switch, so that I can try connecting to old equipment that only speaks outdated algorithms.
43. As a network engineer, I want a failed negotiation to tell me which algorithms the device offered, so that I know whether the legacy switch would help.
44. As a network engineer, I want a clear message when a connection fails (unreachable, refused, authentication failed, timed out), so that I know what to fix.
45. As a network engineer, I want to reconnect a dropped session with one key press in the same tab, so that a flapping link does not cost me my place.
46. As a network engineer, I want a quick-connect box for a host that is not in my inventory, so that I can reach a one-off address.

### Telnet

47. As a network engineer, I want to connect by telnet to devices that have no SSH, so that old gear and lab devices are reachable.
48. As a network engineer, I want telnet to behave correctly with device login prompts and window size, so that it feels the same as SSH.

### Serial console

49. As a network engineer, I want to open a serial session on a COM port, so that I can configure a device through its console port.
50. As a network engineer, I want to pick from a list of detected serial ports with their adapter names, so that I do not guess COM numbers.
51. As a network engineer, I want to set baud rate, data bits, parity, stop bits and flow control, with 9600 8N1 as the default, so that I can match any device.
52. As a network engineer, I want to send a break signal, so that I can interrupt a boot for password recovery.
53. As a network engineer, I want a clear message if the cable is unplugged mid-session, so that the app does not hang.
54. As a network engineer, I want serial settings saved on a host entry, so that a console server port or a known device is one click later.

### Terminal and session management

55. As a network engineer, I want a fast, good-looking terminal with correct colours and cursor handling, so that device output and full-screen tools display properly.
56. As a network engineer, I want several sessions in tabs, so that I can work on many devices at once.
57. As a network engineer, I want to split the window into panes, so that I can watch two devices side by side.
58. As a network engineer, I want each tab to show the host name, role icon and connection state, so that I can find a session at a glance.
59. As a network engineer, I want large outputs such as a full routing table to scroll smoothly without freezing the app, so that the tool keeps up with the device.
60. As a network engineer, I want to search the scrollback, so that I can find a line in long output.
61. As a network engineer, I want copy and paste with sensible keyboard and mouse behaviour, so that moving configuration around is effortless.
62. As a network engineer, I want light and dark themes and an adjustable font, so that the terminal is comfortable for long sessions.
63. As a network engineer, I want to duplicate a session into a new tab, so that I can open a second connection to the same device quickly.

### Session logging

64. As a network engineer, I want every session logged to a file automatically, so that I have a record of what I did and saw.
65. As a network engineer, I want logs organised by workspace, location, host and date-time, so that I can find a past session.
66. As a team member, I want logs stored outside the workspace folder, so that they never end up in the shared repository.
67. As a network engineer, I want to turn logging off for a folder or host, so that sensitive devices are not recorded.
68. As a network engineer, I want the settings to state plainly that logs can contain secrets shown on screen, so that I treat them with care.
69. As a network engineer, I want to open the log folder for the current host from the app, so that I can get to past sessions quickly.

### Highlighting, snippets and paste safety

70. As a network engineer, I want keywords such as down, up, err-disabled, IP addresses and interface names highlighted, so that problems stand out in output.
71. As a network engineer, I want to add my own highlight rules, so that the highlighting fits my vendors.
72. As a network engineer, I want saved command snippets I can send to the current session, so that I do not retype common commands.
73. As a network engineer, I want snippets to use variables filled from the host's fields or prompted when sent, so that one snippet works on many devices.
74. As a team member, I want snippets stored in the workspace, so that the team shares them.
75. As a network engineer, I want a confirmation showing the text before a multi-line paste is sent, so that I do not paste a config into the wrong device.
76. As a network engineer, I want an optional delay between pasted lines, so that slow devices do not drop input.

### Production safety

77. As a network engineer, I want to mark a folder or host as production, staging or lab, so that the app knows what is dangerous.
78. As a network engineer, I want production sessions to have a distinct tab and border colour, so that I cannot mistake them for the lab.
79. As a network engineer, I want multi-line paste on a production device to always ask for confirmation, so that the classic wrong-window mistake is caught.

### Web consoles

80. As a network engineer, I want named web links on a host that open in my default browser, so that a firewall's GUI is one click from its entry.
81. As a network engineer, I want to set a link once on a folder (for example Panorama on the Firewalls folder or an SDN controller on a site), so that every device under it offers it.
82. As a network engineer, I want links to use variables such as the host's management address, so that one definition serves many devices.
83. As a network engineer, I want a host's CLI sessions and web links offered side by side in the side panel, so that I choose the way in that suits the task.

### Import

84. As a network engineer, I want to import hosts from a CSV file with a column-mapping step and a preview, so that I can load an existing spreadsheet.
85. As an automation engineer, I want to import a Nornir inventory (hosts, groups and defaults files), so that the devices my scripts already know appear in the app.
86. As a network engineer, I want an import to report what it created, what it skipped and why, so that I can trust the result.
87. As a network engineer, I want an import to never overwrite existing hosts without asking, so that a re-import is safe.

### Install, update and trust

88. As a Windows user, I want an installer and a portable zip, so that I can install normally or run from a USB stick on a locked-down laptop.
89. As a user, I want to be told when a new version exists and choose when to install it, so that updates never interrupt work.
90. As a security reviewer, I want the app to make no network connections except those I start plus an update check I can disable, so that it can be approved for enterprise use.
91. As a security reviewer, I want no telemetry, no crash reporting to a third party and no AI features, so that device output never leaves the machine.
92. As a security reviewer, I want the source to be open, so that I can audit what handles our credentials.
93. As a user, I want published checksums for each release, so that I can verify a download.
94. As a user, I want the app to start in about a second and stay light on memory, so that it suits a travel laptop.

## Implementation Decisions

### Product and project

- Free and open source under GPLv3, funded by donations only. No paid tier, no hosted service, no contributor agreement.
- Windows is the first-class target for v1. The core must contain no Windows-only assumptions so that macOS and Linux builds can follow.
- The codename `netterm` is used for the repository and internal identifiers. The user-visible name is defined in one place so that renaming is a small change.
- The repository stays private until a name is chosen, then moves to a public organisation.

### Stack

- Tauri 2 desktop shell, Rust backend, Svelte 5 with TypeScript for the interface, xterm.js for the terminal.
- SSH uses `russh` with its legacy-algorithm features enabled and the `ring` crypto backend, which avoids extra native build tooling on Windows. The `russh` version is pinned because its API has been changing roughly monthly.
- Serial uses the `serialport` crate with a dedicated reader thread per port feeding the async runtime. The async serial crate is avoided because of known Windows issues.
- Telnet is implemented in the core, including option negotiation for window size, terminal type, echo and suppress-go-ahead.
- Built from scratch. Existing Tauri SSH clients may be studied for architecture; code is only reused where its licence is compatible with GPLv3.

### Architecture

- A UI-free Rust core holds all behaviour. The Tauri layer is a thin adapter that exposes the core to the interface, and the interface only renders. This boundary is the single test seam.
- Core modules:
  - **Workspace:** loads a workspace folder, resolves inherited settings, writes changes back, watches for external changes.
  - **Index:** an in-memory search index over the workspace, rebuilt from the files. The files are the only source of truth.
  - **Importers:** CSV and Nornir.
  - **Credential store:** an interface with an OS-keychain implementation and an in-memory implementation.
  - **Session manager:** opens, tracks and closes sessions over three transports (SSH, telnet, serial) behind one session interface: bytes in, bytes out, resize, close, state changes.
  - **Jump chaining:** opens an SSH session through one or more intermediate SSH hosts using forwarded channels.
  - **Output pipeline:** batches session output for delivery to the interface.
  - **Session logger:** writes session output to log files.
  - **Link and snippet templating:** fills variables from resolved host fields.
- App-level state (the list of known workspaces, window layout, theme, update settings, known host keys) is stored in the per-user application data folder, never in a workspace.

### Workspace format

- A workspace is a folder of YAML files. Directories are the hierarchy.
- Each directory may hold one settings file for that folder (description, inherited settings, web links).
- Each host is one file, so that edits to different hosts never conflict in git.
- Workspace-wide definitions (custom roles, saved filters, snippets, highlight rules) live in files at the workspace root.
- Credential profiles are referenced by name only. No secret is ever written to a workspace file.

### Host and settings model

- Built-in host fields: name, management address, vendor, platform, model, serial number, site, building, room, rack, rack unit, role, description, tags.
- Custom fields: arbitrary key/value pairs.
- Inheritable settings: default protocol, port, credential profile, jump host, legacy-crypto switch, logging on/off, environment (production, staging, lab), web links, serial parameters.
- Resolution rules:
  - Single-value settings: the nearest definition wins (host, then its folder, then each parent up to the root).
  - Web links: merged by name down the tree; a child definition with the same name replaces the parent's.
  - Tags: additive down the tree.
- The resolved host carries the origin of each effective setting so the interface can show where it came from.
- Roles: a built-in starter list (core, distribution, access, edge/WAN router, firewall, load balancer, wireless controller, access point, out-of-band/console server, server), extendable per workspace. Each role has an icon and a colour.

### Credentials

- A profile holds a username, one of password / private key reference / agent, and an optional enable secret.
- Secrets are stored in the OS keychain (Windows Credential Manager on Windows), keyed by workspace and profile name.
- SSH agent support covers the Windows OpenSSH agent and Pageant.
- A host with no resolvable profile, or a profile with no stored secret, results in an interactive prompt.

### SSH behaviour

- Authentication methods: password, public key, agent, keyboard-interactive.
- Host keys: trust on first use with a prompt showing the fingerprint; a changed key blocks the connection with a warning and requires an explicit decision. Known keys are stored per user, outside workspaces.
- Legacy crypto is best effort. The switch adds the outdated algorithms `russh` supports (SHA-1 key exchanges, CBC ciphers, 3DES, SHA-1 MAC, DSA host keys) to the offer for that host. The project does not fork or patch `russh` for algorithms it lacks; MD5-based MACs are not supported.
- A failed algorithm negotiation reports the algorithms the device offered.
- A jump host is a reference to another host in the same workspace and is resolved with the same rules, so chains of any length work.

### Terminal output path

- Session output travels from the core to the interface over Tauri channels carrying raw bytes. The Tauri event system is not used for terminal data.
- The output pipeline flushes immediately when a session has been idle, to keep keystroke echo latency low, and otherwise batches on a short timer (about 8–16 ms) or size threshold (about 32–64 KB).
- Flow control: the interface acknowledges bytes as the terminal consumes them, and the core pauses reading from the session when the interface falls behind.
- The WebGL renderer is attached only to visible terminals, because browsers limit the number of WebGL contexts, and context loss is handled. Hidden terminals buffer their output and are fed when shown.

### Interface

- Layout: inventory tree on the left, session tabs with split panes in the centre, a host side panel showing fields, connection options and web links.
- Double-clicking a host opens a session with its default protocol. The side panel offers every other way in.
- Command palette on Ctrl+K for host search and connect; a workspace switcher; a quick-connect entry for hosts not in the inventory.
- Tabs show host name, role icon and colour, connection state and environment colour.
- Light and dark themes; adjustable terminal font and size.

### Logging

- On by default; switchable through inheritance.
- Logs are written under a per-user log folder outside any workspace, organised by workspace, tree path, host and session start time.
- Log content is plain text with terminal control sequences removed.

### Snippets, highlighting and paste safety

- Snippets are named pieces of text stored in the workspace, sent to the active session. Variables are filled from the resolved host's fields; unknown variables are prompted for.
- Highlighting applies a built-in rule set plus user-defined regular-expression rules from the workspace to terminal output as decoration; it never alters the bytes or the log.
- A multi-line paste shows the text and asks for confirmation. The confirmation can be turned off for non-production hosts and cannot be turned off for production hosts. An optional per-line delay is configurable.

### Web links

- A link has a name and a URL template. Only `http` and `https` URLs are opened, always in the default browser.
- The app does not embed web consoles and stores no web logins.

### Import

- CSV: the user maps columns to fields, previews the result and chooses a target folder.
- Nornir: reads the hosts, groups and defaults files of a simple inventory. Hosts land in a chosen target folder; group membership becomes tags; extra data becomes custom fields.
- Passwords found in an imported file are never written to the workspace; the import report lists them as skipped.
- Existing hosts are never overwritten without confirmation. Every import ends with a report of created, skipped and failed entries.

### Packaging and updates

- Releases are published as an installer and a portable zip, with SHA-256 checksums.
- Early releases are unsigned, with a note explaining the Windows SmartScreen prompt. Signing through Azure Artifact Signing starts at the public launch.
- The updater signing key is created before the first release and backed up; losing it strands installed copies.
- The installed build checks for updates and offers them; the check can be disabled. The portable build only notifies.

### Build order

1. Terminal core: one SSH session in a window with the batched binary output path.
2. Inventory: workspace format, tree, fields, roles, inheritance, search and palette.
3. Credentials and connection options: keychain profiles, agent, jump hosts, legacy-crypto switch, host-key handling.
4. Telnet and serial.
5. Session quality: tabs, split panes, logging, highlighting, snippets, paste safety, production cues.
6. Web links, multiple workspaces, CSV and Nornir import.
7. Packaging: installer, portable zip, update check.

## Testing Decisions

- A good test exercises the core's public behaviour and asserts on what a user would observe: the resolved settings of a host, the bytes that arrive from a device, the contents of a log file, the entries an import created. Tests do not assert on internal structures, private functions or the order of internal calls.
- There is one test seam: the public API of the UI-free core. Tests fall into two areas.
  - **Workspace tests** load fixture workspace folders from disk and check resolution (nearest-wins, link merging, additive tags, setting origins), search, writing changes back, reacting to external file changes, and CSV and Nornir import including the report and the no-overwrite rule. These need no network.
  - **Session tests** open real sessions through the session manager:
    - a containerised OpenSSH server and a telnet server on every run, covering each authentication method, host-key first-use and changed-key handling, jump chains, legacy-algorithm negotiation against a server configured down to old algorithms, resize, reconnect, large-output throughput with flow control, and logging;
    - a slower suite against network operating systems on a containerlab server (Nokia SR Linux, Arista cEOS, FRR), plus Cisco and Juniper images if licences are available;
    - a virtual serial port pair for serial sessions, including parameters, break and unplug.
- The credential store is substituted with its in-memory implementation in tests. The keychain implementation gets a small set of tests of its own on Windows.
- The interface has no unit tests in v1. One end-to-end smoke test launches the app, opens a host and checks that a prompt appears.
- The app states which device platforms it has been tested against and claims no more.
- There is no prior art: this is a new codebase, and these tests establish the pattern.

## Out of Scope

Planned for later phases:

- **v1.x:** jobs that run a snippet or request against many devices with collected, searchable results; NetBox read-only one-way sync; a Python bridge using the user's own interpreter, with the inventory readable as a Nornir inventory; NAPALM-based enrichment of hosts already in the inventory; multi-send to several terminals.
- **v2:** NETCONF and RESTCONF client with saved request collections; later a YANG browser.
- **v3:** SFTP/SCP file browser; one-click temporary TFTP, FTP and SFTP servers.
- Later, undated: port forwarding and SOCKS; config backup and diff; expect-style scripting of live sessions; quick tools (ping, traceroute, subnet calculator, OUI lookup); a topology map generated from neighbour data; an app lock or master password; macOS and Linux releases; `winget` listing; code signing.

Not planned:

- Import from PuTTY, SecureCRT or MobaXterm.
- Subnet scanning or discovery of unknown devices.
- A manual diagram editor.
- Telemetry, crash reporting to third parties, AI features.
- Hosted sync, accounts or any cloud service.
- Embedded web consoles or storage of web logins.
- Embedding a Python interpreter.
- Forking or patching the SSH library to add algorithms it lacks.

## Further Notes

- **Unverified claims from research,** to be settled by the first build step and the session tests:
  - legacy-algorithm support in `russh` was read from documentation and source, not tested against a device;
  - behaviour with very short RSA host keys and small key-exchange groups on old gear is unknown;
  - terminal throughput over the Tauri channel on Windows has no published measurement;
  - serial DTR/RTS defaults, high COM port numbers and break behaviour on Windows need testing with real adapters.
- **Decisions made while writing this spec that were not discussed** and should be reviewed:
  - host-key handling (trust on first use, known keys stored per user);
  - one file per host, with a settings file per folder;
  - link merging by name and additive tags;
  - logs as plain text with control sequences removed;
  - the enable secret is stored in the profile but v1 has no automatic login sequence that sends it;
  - Nornir groups map to tags, and passwords in imported files are skipped;
  - the multi-line paste confirmation cannot be disabled for production hosts;
  - the portable build notifies of updates but does not self-update.
- **Costs:** none in v1. Signing at public launch is about $10 a month; a macOS release later needs the $99 a year Apple Developer Program.
