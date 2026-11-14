# netterm

A terminal for network engineers: SSH, telnet and serial sessions to the
devices in an inventory kept as plain YAML files (a folder you can put in
git), with credential profiles in the Windows Credential Manager, jump
hosts, session logs, snippets, keyword highlighting and paste safety for
production devices. Free and open source under the GPL-3.0.

Windows 10 and 11 (64-bit) for now. The app needs Microsoft Edge WebView2,
which Windows 11 includes; the installer fetches it on older systems if it
is missing. The portable copy cannot install it: on a computer without
WebView2 it does not start.

## Download and install

Each release has these files:

| File                                 | What it is                                                                  |
| ------------------------------------ | --------------------------------------------------------------------------- |
| `netterm_<version>_x64-setup.exe`    | The installer. Installs for your user only; no administrator rights needed. |
| `netterm_<version>_x64-portable.zip` | The portable copy: unpack it anywhere (a USB stick) and run `netterm.exe`.  |
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

Open **Preferences** (the gear in the workspace header) and clear **Check for
a new version at start**. With it off, the app makes no network connection
that you did not start (sessions, web links, **Check now**). The setting is
kept per user in the app's data folder (`preferences.yaml`, key
`check_for_updates`).

The app sends no telemetry and has no crash reporting. The update check is a
plain request for the release manifest (`latest.json`) and sends nothing
about you, your machine or your inventory.

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
`npm run app:build`), and `docs/releasing.md` for making a release.

## License

netterm is free software under the GNU General Public License, version 3 only
(`GPL-3.0-only`); see [LICENSE](LICENSE). It is funded by donations, with no paid
tier, no hosted service and no contributor agreement.
