# netterm

A terminal for network engineers: SSH, telnet and serial sessions to the
devices in an inventory kept as plain YAML files (a folder you can put in
git). Free and open source under the GPL-3.0.

- **Sessions:** tabs and split panes, jump hosts, session logs, keyword
  highlighting, snippets with variables filled from the host, and paste
  safety for production devices.
- **Multi-send:** type into a group of terminals at once; joining a
  production device asks first.
- **Credentials:** profiles are kept in the Windows Credential Manager, or
  in the app's own encrypted vault behind a master password (your choice;
  on Linux the vault is the only store). Workspace files hold only a
  profile's name.
- **Jobs:** run a snippet on a folder, a saved filter or one host, with
  per-device output you can search, compare and save; nothing is asked
  mid-run, and a list beforehand shows which hosts will be skipped and why.
- **Config backup:** as a job or each time you log in, kept as versions
  with a diff between any two.
- **Python:** the inventory and logins as a Nornir inventory for your own
  Netmiko, scrapli and NAPALM scripts, run from the app or from a shell,
  with hosts behind jump hosts reached through the app's own jump chains.
  Python is optional; nothing else needs it.
- **Fill in from devices:** vendor, model, serial number and OS version
  fetched with NAPALM, reviewed before anything is written.

The window is a thin rail of icons on the left (Hosts, Snippets, Jobs,
Config backups, Credentials, and Preferences at its foot), one side view
that it switches and folds, and the terminals in tabs and split panes.
Job runs, config history, Preferences, the imports and the other tools
open as tabs beside the terminals, and a job run or a config history can
open in a window of its own. [docs/interface.md](docs/interface.md)
describes the window, lists the shortcuts and says where everything is.

Releases up to 0.1.2 have the sessions and the inventory only; everything
else in the list came with 0.2.0, and the redesigned interface with 0.3.0. Releases so far are pre-releases: not
code-signed, and without the automatic updates described below (install
the next version over the old one by hand). [CHANGELOG.md](CHANGELOG.md) lists what
each release added, and [ROADMAP.md](ROADMAP.md) says what is built, what
is not yet verified and what comes next.

Windows 10 and 11 (64-bit). The app needs Microsoft Edge WebView2,
which Windows 11 includes; the installer fetches it on older systems if it
is missing. The portable copy cannot install it: on a computer without
WebView2 it does not start.

Linux packages (.deb, .rpm, AppImage) are built too, but the app has not
been checked on a real Linux desktop yet, so Linux is not claimed as
supported; see [docs/linux.md](docs/linux.md) for how it differs there.

## Download and install

Each release has these files:

| File                                 | What it is                                                                  |
| ------------------------------------ | --------------------------------------------------------------------------- |
| `netterm_<version>_x64-setup.exe`    | The installer. Installs for your user only; no administrator rights needed. |
| `netterm_<version>_x64-portable.zip` | The portable copy: unpack it anywhere (a USB stick) and run `netterm.exe`.  |
| `netterm_<version>_amd64.deb`        | Debian and Ubuntu package (from the next release; not desktop-checked yet). |
| `netterm-<version>-1.x86_64.rpm`     | Fedora, RHEL and openSUSE package (the same).                               |
| `netterm_<version>_amd64.AppImage`   | One Linux file that runs without installing (the same).                     |
| `SHA256SUMS.txt`                     | The SHA-256 checksum of every file, to check a download.                    |
| `latest.json`, `*.sig`               | Used by the app's update check; you do not need them.                       |

### Checking a download

In PowerShell, compare the file's checksum with its line in `SHA256SUMS.txt`:

```powershell
Get-FileHash .\netterm_0.1.0_x64-setup.exe -Algorithm SHA256
```

Get the checksums from the release page itself, not from a copy sent with the
download.

### "Windows protected your PC" (SmartScreen)

Early releases are not code-signed (signing starts at the public launch).
The first time you run the installer or `netterm.exe`, Windows SmartScreen
may show a blue "Windows protected your PC" window saying the app is from an
"Unknown publisher". Windows shows this for any unsigned program; it does
not mean a virus was found. To go on:

1. Check the file's SHA-256 checksum against `SHA256SUMS.txt` (above).
2. Click **More info**, then **Run anyway**.

If the downloaded file is blocked instead (no **Run anyway** button), open
its **Properties**, tick **Unblock** and click **OK**. Your organisation may
not allow unsigned programs at all; ask your IT team in that case.

## Updates

The installed app checks for a new version once when it starts. When one is
out, a notice in the corner says so; nothing is downloaded or installed until
you click **Install and restart** (open sessions are closed, so it asks
first when you have any). **Later** hides the notice until the next start.

The portable copy only tells you a new version exists and links to its
download page; it never replaces itself. Unpack the new zip over the old
folder.

Updates are signed: the app installs only a download signed with the
project's release key.

### Turning the update check off

Open **Preferences** (the gear at the foot of the rail, or Ctrl+,) and clear **Check for
a new version at start**. With it off, the app makes no network connection
that you did not start (sessions, web links, **Check now**). The setting is
kept per user in the app's data folder (`preferences.yaml`, key
`check_for_updates`).

The app sends no telemetry and has no crash reporting. The update check is a
plain request for the release manifest (`latest.json`) and sends nothing
about you, your machine or your inventory.

## The headless binary

`netterm-cli.exe` is installed beside `netterm.exe` (and is in the portable
zip). It has no window and does not need the app running. It prints a
workspace's inventory as JSON, each host with the settings that apply to it
(inheritance resolved), its jump hosts, folders, tags and custom fields;
credential profiles appear by name only, and custom fields named like a
secret (password, key, community, token) are left out:

```powershell
netterm-cli inventory C:\Work\inventory
netterm-cli inventory C:\Work\inventory --folder Europe/Madrid
netterm-cli inventory C:\Work\inventory --filter "Arista in Madrid"
netterm-cli inventory C:\Work\inventory --host core-r1.yaml --host Lab/sw1.yaml
```

Problems with workspace files are printed as warnings and do not stop the
export. It exits with 1 (and says why) for a missing workspace or an
unknown folder, filter or host, and with 2 for a command line it does not
understand. `netterm-cli --help` lists the options.

It also hands out logins and opens tunnels, for scripts:

```powershell
netterm-cli credentials C:\Work\inventory Europe/Madrid/core-r1.yaml
netterm-cli secret C:\Work\inventory netbox-token
netterm-cli tunnel C:\Work\inventory --folder Europe/Madrid
```

- `credentials` prints the login of the credential profile a host uses, and
  `secret` a named secret of the workspace, as JSON. They come from the
  running app when it is open and its vault unlocked; otherwise from the
  keychain, or from the vault after asking its master password on the
  terminal. A password is never taken from the command line or the
  environment, and nothing is written to disk.
- `tunnel` opens a port on `127.0.0.1` for each host behind jump hosts,
  through the jump chain the app would use, and serves it until its
  standard input ends. It asks no host-key or login question: a jump host
  or device whose key the app has not trusted is refused, with the reason
  in its output.

## Python and Nornir

The `python` folder beside `netterm.exe` (and in the portable zip) holds a
small pure-Python package that presents a workspace as a Nornir inventory,
with the logins of your credential profiles, and reads named secrets. Its
version always matches the app's; install it from that file:

```powershell
pip install "$env:LOCALAPPDATA\netterm\python\netterm-<version>-py3-none-any.whl"
```

Then use the `NettermInventory` plugin in `InitNornir`. Hosts behind jump
hosts are reached through tunnels the plugin opens with `netterm-cli`, so
scripts run unchanged; a device must have been connected to once in the
app, so that its host key is trusted. See
[python/README.md](python/README.md) for the options, the mapping and the
limits.

The app can also do this for you: in **Preferences**, Python sets up an
environment (after showing what it will install) or uses your own
interpreter, and then **Run script…** on a folder, filter or host runs a
`.py` file of the workspace as a job, and **Fill in from devices** asks
the devices for their vendor, model, serial number and OS version.

## Tested platforms

netterm claims support only for the device platforms below, each verified by the
containerlab device suite ([docs/containerlab.md](docs/containerlab.md))
at the version shown. Other platforms may work but have not been tested.

| Platform                 | Version       | SSH | Telnet     | Verified   |
| ------------------------ | ------------- | --- | ---------- | ---------- |
| Nokia SR Linux           | 24.10.1       | yes | not tested | 2026-11-14 |
| Arista cEOS              | 4.36.0.1F     | yes | yes        | 2026-11-14 |
| FRRouting                | 10.2.1        | yes | not tested | 2026-11-14 |
| Cisco IOS-XE (CSR 1000v) | 17.03.08      | yes | not tested | 2026-11-14 |
| Juniper cJunos Evolved   | 25.4R1.13-EVO | yes | not tested | 2026-11-14 |

SSH covers login with a password, a command and its output, host-key
checking, a refused password, resizing, large output with flow control and
session logs, each through an SSH jump host, and jobs and config backups
with the built-in platform definition (paging off, privileged mode, the
whole configuration, backup on login). Cisco NX-OS has not been tested.

## Building from source

For the layout and commands (`npm install`, `npm run check`,
`npm run app:build`; `npm run test:python` for the Python bridge and
`npm run test:linux` for the Linux checks), `docs/linux.md` for Linux and
`docs/releasing.md` for making a release.

## License

netterm is free software under the GNU General Public License, version 3 only
(`GPL-3.0-only`); see [LICENSE](LICENSE). It is funded by donations, with no paid
tier, no hosted service and no contributor agreement.
