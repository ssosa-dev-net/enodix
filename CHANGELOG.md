# Changelog

What each release added, newest first. The numbers in brackets are tickets
in the project's tracker.

## Unreleased

Nothing yet.

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
