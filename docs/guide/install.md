# Install, update and uninstall

How to get enodix onto a computer, check what you downloaded, keep it up to
date and take it off again, including the switches an IT team needs for a
silent install. Download the files from the repository's **Releases** page.

## What to download

Each release has these files:

| File                                | What it is                                                                  |
| ----------------------------------- | --------------------------------------------------------------------------- |
| `enodix_<version>_x64-setup.exe`    | The installer. Installs for your user only; no administrator rights needed. |
| `enodix_<version>_x64-portable.zip` | The portable copy: unpack it anywhere (a USB stick) and run `enodix.exe`.   |
| `enodix_<version>_amd64.deb`        | Debian and Ubuntu package.                                                  |
| `enodix-<version>-1.x86_64.rpm`     | Fedora, RHEL and openSUSE package.                                          |
| `enodix_<version>_amd64.AppImage`   | One Linux file that runs without installing.                                |
| `SHA256SUMS.txt`                    | The SHA-256 checksum of every file, to check a download.                    |
| `latest.json`, `*.sig`              | Used by the app's update check; you do not need them.                       |

On Linux, see [enodix on Linux](linux.md) for what is different there.

## Windows

enodix runs on Windows 10 and 11 (64-bit). It needs Microsoft Edge
WebView2, which Windows 11 includes; the installer downloads it when it is
missing.

On a computer with neither WebView2 nor internet access (a Windows Server
jump box, say), install Microsoft's Evergreen Standalone WebView2 runtime
first. The portable copy needs it too: it cannot install WebView2, and
without it the app does not start.

Run the installer, or unpack the portable zip and run `enodix.exe`. The
installer puts the program in `%LOCALAPPDATA%\enodix` and adds Start menu
and desktop shortcuts.

### Check a download

In PowerShell, compare the file's checksum with its line in `SHA256SUMS.txt`:

```powershell
Get-FileHash .\enodix_0.1.0_x64-setup.exe -Algorithm SHA256
```

Get the checksums from the release page itself, not from a copy sent with the
download.

### "Windows protected your PC" (SmartScreen)

The releases are not code-signed yet. The first time you run the installer
or `enodix.exe`, Windows SmartScreen may show a blue "Windows protected
your PC" window saying the app is from an "Unknown publisher". Windows shows
this for any unsigned program; it does not mean a virus was found. To go on:

1. Check the file's SHA-256 checksum against `SHA256SUMS.txt` (above).
2. Click **More info**, then **Run anyway**.

If the downloaded file is blocked instead (no **Run anyway** button), open
its **Properties**, tick **Unblock** and click **OK**. Your organisation may
not allow unsigned programs at all; ask your IT team in that case.

## Linux

The packages (.deb, .rpm, AppImage) install the same app. [enodix on
Linux](linux.md) says how it differs from the Windows one (logins, updates,
where files are) and how to remove it.

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

### Turn the update check off

Open **Preferences** (the gear at the foot of the rail, or Ctrl+,) and clear
**Check for a new version at start**. With it off, the app makes no network
connection that you did not start (sessions, file transfers, jobs, scripts,
a Python setup you confirmed, **Check now**; web links open in your
browser). The setting is kept per user in the app's data folder
(`preferences.yaml`, key `check_for_updates`).

The app sends no telemetry and has no crash reporting. The update check is a
plain request for the release manifest (`latest.json`) and sends nothing
about you, your machine or your inventory (the server sees your address,
as with any connection). [Trust and privacy](../trust.md) lists every
connection the app can make.

## Install and uninstall silently

For deployment scripts, the installer takes these switches (`uninstall.exe`
takes `/S` and `/P`):

| Switch      | What it does                                                                                      |
| ----------- | ------------------------------------------------------------------------------------------------- |
| `/S`        | Silent: no window, no questions. A silent install also makes a desktop shortcut, unless `/NS`.    |
| `/P`        | Passive: shows progress only, asks nothing (the mode updates use). Also makes a desktop shortcut. |
| `/NS`       | No shortcuts: neither the Start menu one nor the desktop one.                                     |
| `/D=<path>` | Installs into `<path>` instead of `%LOCALAPPDATA%\enodix`. It must come last, without quotes.     |

A running enodix or `enodix-cli.exe` is closed without asking in silent
and passive mode. A silent uninstall (`uninstall.exe /S`) never deletes your
data or saved logins. It returns at once, while a copy of it carries on from
the temp folder for a few seconds.

## Uninstall

Uninstall enodix from **Settings, Apps, Installed apps** (or run
`uninstall.exe` in its folder, `%LOCALAPPDATA%\enodix`). It closes enodix,
and `enodix-cli.exe` if a script is using it, then removes the program
folder (the app, `enodix-cli.exe`, the Python wheel, the licence and the
third-party notices), the Start menu and desktop shortcuts and the entry in
Installed apps. Files you put in the program folder yourself are left there.
On Linux the package manager removes it ([enodix on Linux](linux.md),
"Remove it").

Your data stays, so a later install finds it again, unless you tick
**Also delete settings, saved logins and the vault, ...** on the uninstall
page. That deletes:

- `%APPDATA%\dev.enodix.app`: settings, the vault, known host keys, the
  list of workspaces and the Python choice;
- `%LOCALAPPDATA%\dev.enodix.app`: session logs, config backups, kept job
  results, the Python environment and the window's web view data;
- the saved logins (credential profiles and named secrets) in Windows
  Credential Manager, the entries whose names start with `dev.enodix.app`;
- the registry key `HKCU\Software\Sergio Sosa\enodix`, where the installer
  remembers the program folder.

What always stays: your workspace folders, files you downloaded from
devices, and a Python bridge you installed with `pip`. If you uninstalled
without ticking the box, delete the two folders above by hand, and the
Credential Manager entries in **Control Panel, Credential Manager, Windows
Credentials**. Running `enodix-cli clear-keychain --yes` before you
uninstall removes those entries for you (see [The headless
binary](cli.md#remove-every-saved-login)).

## Upgrading from 0.4 or earlier

Config backups, kept job results and the Python environment are large and
only make sense on one computer, so from 0.5.0 they live in
`%LOCALAPPDATA%\dev.enodix.app`, not in the roaming `%APPDATA%`, which a
company's roaming profiles copy at every logon. The first start of the new
version moves them there, once, before it shows anything:

- On one drive the move is a rename, done whole or not at all. When the
  roaming folder is on another drive (redirected to a server), the backups
  and results are copied, each file checked, and only then removed from
  the old place. The Python environment is not copied but goes on working
  where it is until you set it up again (Preferences, Python), which makes
  it in the local folder and removes the old one.
- A move that is cut short (the computer switched off, a full disk) loses
  nothing: the next start finishes it or starts it again. A move that
  fails leaves the data where it was, and the app uses it there and says
  so in a notice.
- If both folders hold backups or results (an older version was run after
  the move), neither is changed: the app uses the local one and says where
  the other is, so you can look in it and remove it.
- While another copy of enodix runs, a starting copy moves nothing: the
  move waits for a start when it runs alone.

On Linux everything lives in `~/.local/share/dev.enodix.app` and nothing moves.
