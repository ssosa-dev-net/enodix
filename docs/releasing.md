# Making a release

A release is built with `npm run release` (`scripts/release.mjs`): on
Windows the Windows files, on Linux the Linux packages. From Windows,
`npm run release:linux` (`scripts/release-linux.mjs`) builds the Linux
packages in WSL and puts them beside the Windows files. The scripts build
and check the files; they never upload, tag or publish anything. You
publish by hand, as described below.

## Pre-releases (no release key yet)

Until the release key exists (below), releases are unsigned pre-releases:

```
npm run release -- --unsigned
npm run release:linux -- --unsigned
```

These need no key and no environment variables and write to
`target/dist/<version>-unsigned/`: the installer, the portable zip, the
Linux packages and `SHA256SUMS.txt`, with no `.sig` and no `latest.json`.
The app in them has no updater and no release server, so it never checks
for or installs an update; people install the next version by hand.
Versions 0.1.0 to 0.4.1 were made this way and published on the tracker's
release page as pre-releases. The first signed release starts the updater:
copies of a pre-release cannot update themselves to it.

Steps 1, 2, 4 and 5 of "Each release" below apply as they are; step 3 is the
two commands above, and there is no `latest.json` to publish.

## What a release contains

`target/dist/<version>/`:

| File                                 | Purpose                                                                          |
| ------------------------------------ | -------------------------------------------------------------------------------- |
| `netterm_<version>_x64-setup.exe`    | NSIS installer, per user (no administrator rights); installs `netterm-cli.exe` and `LICENSE` beside the app and the Python wheel in `python\` (see "The installer" below). |
| `netterm_<version>_x64-setup.exe.sig`| The updater signature of the installer (made with the release key).              |
| `netterm_<version>_x64-portable.zip` | `netterm.exe`, `netterm-cli.exe`, `python\netterm-<version>-py3-none-any.whl` (the Python bridge), `portable.txt` (makes the copy portable: it only notifies), README.|
| `latest.json`                        | The update manifest the app checks (Tauri updater format, plus `page`).          |
| `netterm_<version>_amd64.deb`        | Debian/Ubuntu package: `/usr/bin/netterm`, `/usr/bin/netterm-cli`, the wheel in `/usr/lib/netterm/python/`. |
| `netterm-<version>-1.x86_64.rpm`     | Fedora/RHEL/openSUSE package, the same files.                                    |
| `netterm_<version>_amd64.AppImage`   | One file that runs without installing; `netterm-cli` and the wheel are inside it. |
| `SHA256SUMS.txt`                     | SHA-256 of every other file, in `sha256sum` format.                              |

Each platform's build replaces only its own files in the folder, and
`SHA256SUMS.txt` is rewritten to list everything there, so the order of the
two builds does not matter. The Linux packages are neither signed for the
updater nor listed in `latest.json`: on Linux the app only announces a new
version (see below), and people update with their package manager or a new
AppImage.

The script also checks the installer's signature against the public key
the app is built with, the same way the updater will, and stops if they do
not match (a release signed with the wrong key would strand every installed
copy).

Releases are not code-signed (Authenticode) yet; the README explains the
SmartScreen prompt. Signing through Azure Artifact Signing starts at the
public launch.

## Once, before the first release: the updater signing key

The installed app installs an update only if it is signed with the release
key: the public half is built into the app, the private half signs each
release. **If the private key or its password is lost, installed copies can
never update again**: every user would have to download and reinstall by
hand. If it leaks, anyone who can serve a manifest to users can push them
software. So:

1. Create it outside the repository, with a strong password:

   ```
   npx tauri signer generate -w "%USERPROFILE%\.netterm-release\netterm-updater.key"
   ```

   This writes `netterm-updater.key` (private, encrypted with the password)
   and `netterm-updater.key.pub` (public).

2. Keep the password in your password manager.

3. Back up the private key file **and** the password to two places that are
   not this computer, for example the password manager (as an attachment)
   and an encrypted USB stick kept elsewhere. Check that the backup opens.

4. Never commit the private key. `.gitignore` ignores `*.key`, and the
   release script refuses a key inside the repository.

5. Copy the public key into the repository and commit it:

   ```
   copy "%USERPROFILE%\.netterm-release\netterm-updater.key.pub" src-tauri\updater-key.pub
   ```

   The release script builds it into the app. (It can also be given with
   `NETTERM_UPDATER_PUBKEY`.)

6. Write down here where the key and its backups are (not the password):

   - Private key: _not created yet_
   - Backups: _not created yet_

Also decide once **where `latest.json` is published**: one address that
never changes, because every installed copy keeps asking it. It must be
https with a certificate Windows trusts (the app uses the Windows
certificate store; a self-signed certificate on a private server fails the
check, harmlessly but always). On GitHub,
`https://github.com/<owner>/<repo>/releases/latest/download/latest.json`
always serves the newest release's file; check that your host offers an
equivalent, or publish the file at a fixed address yourself.

## Each release

1. Set the version in `Cargo.toml` (`[workspace.package] version`) and
   `package.json` (the script refuses if they differ). Versions are semver;
   the app offers only a higher version than its own. In `CHANGELOG.md`,
   rename "Unreleased" to the version and start a new "Unreleased" above it.
2. `npm run check`, `npm run test:python` (the Python bridge: needs Python
   3.10+ and, the first time, the network for Nornir), and
   `npm run app:build && npm run smoke`.
3. Build the Windows files, in a shell where the key is set (PowerShell shown):

   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.netterm-release\netterm-updater.key"
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "<from the password manager>"
   $env:NETTERM_UPDATE_ENDPOINT = "https://<host>/<owner>/netterm/releases/latest/download/latest.json"
   $env:NETTERM_RELEASE_URL = "https://<host>/<owner>/netterm/releases/download/v0.2.0/"
   $env:NETTERM_RELEASE_PAGE = "https://<host>/<owner>/netterm/releases/tag/v0.2.0"   # optional
   $env:NETTERM_RELEASE_NOTES = "Serial break works on FTDI adapters."                 # optional
   npm run release
   ```

   `NETTERM_UPDATE_ENDPOINT` is built into the app: keep it the same for
   every release. `NETTERM_RELEASE_URL` is where this release's files will
   be downloadable; `latest.json` points the updater at the installer there.
   The first build downloads NSIS into Tauri's cache.

   Then the Linux packages, in the same shell (only
   `NETTERM_UPDATE_ENDPOINT` is used; no key). Build both from the same
   commit: the folder keeps the other platform's files, and a failed build
   leaves the other platform's files there.

   ```powershell
   npm run release:linux
   ```

   It copies the working tree into WSL (`~/netterm-linux-check`, see
   `docs/linux.md` for the packages the distribution needs), runs
   `npm ci` and `npm run release` there and copies the .deb, .rpm and
   .AppImage into `target/dist/<version>/`. On a Linux machine, run
   `npm run release` there instead. The first build downloads the AppImage
   tools into Tauri's cache.

4. Check the installer, on a Windows computer where netterm is not
   installed for your user (the script stops otherwise), with the previous
   release's installer:

   ```
   npm run test:installer -- -Old <folder>\netterm_<previous>_x64-setup.exe
   ```

   `scripts/check-installer.ps1` installs the previous release, upgrades it
   to this one while the installed `netterm-cli.exe` runs, uninstalls
   (again with it running), installs this one fresh and uninstalls it, and
   upgrades a copy in another folder in passive mode, all silently or
   passively. After each step it lists the install folder, the uninstall
   entry, the vendor registry keys, the shortcuts and the version details,
   then puts the computer back. Check that each upgrade leaves one wheel,
   `LICENSE` and the new `netterm-cli.exe`, that the running one was
   closed (Windows takes about 30 seconds to end it), that each uninstall
   leaves no install folder, entry or shortcut, and that the last lines say
   the data folders are as they were. It never
   starts the app and never ticks the data box: the wizard's pages, the box
   and the updater's own install are not exercised. Paste its output into
   the release's ticket.

   Then the install-and-run check, on a clean Windows: in Windows Sandbox
   (the feature turned on once, see the testing notes), with
   nothing of yours at risk:

   ```
   npm run test:installer:sandbox -- --old <folder>\netterm_<previous>_x64-setup.exe
   ```

   It runs the check above inside the sandbox, then installs this release,
   starts the app and waits for its window, puts two saved logins of the
   app's and one of another program in Credential Manager, upgrades over
   itself in passive mode as the updater would, and uninstalls through the
   uninstaller's window with "Delete the application data" ticked. It
   fails if anything of the app's is left: the install folder, the
   uninstall entry, the vendor key, shortcuts, the data folders or the
   app's Credential Manager entries (the other program's must stay). The
   log is printed and kept in `target/sandbox/installer-<time>/out/`. Its
   window is on a desktop of its own; it takes about two minutes. Paste its
   output into the release's ticket too.

5. Publish by hand: create the release (tag `v<version>`), upload every file
   from `target/dist/<version>/`, and paste the version's section of
   `CHANGELOG.md` and `SHA256SUMS.txt` into the release notes. Make `latest.json` reachable at `NETTERM_UPDATE_ENDPOINT` **last**,
   once the installer can be downloaded.

6. Check: an installed copy of the previous version shows the notice
   (**Check now** in Preferences), and installing works. On Linux, install
   the .deb in WSL and run `npm run test:linux-desktop` (also with
   `--backend x11` and `--app <the AppImage>`), then check what it cannot
   on a real desktop with the list in `docs/linux.md`.

The release build replaces `target/release/netterm.exe` with one that
has the updater; run `npm run app:build` again before the smoke test.

## The installer

Tauri generates the NSIS installer; `scripts/release.mjs` adds to it:

- `src-tauri/installer/hooks.nsh` (Tauri's `installerHooks`): a running
  `netterm-cli.exe` is closed like the app before installing or
  uninstalling; installing removes the wheels of other versions from
  `python\`, uninstalling every wheel there, then `python\` and the program
  folder if empty; with "Delete the application data" ticked, the
  uninstaller runs `netterm-cli clear-keychain --yes` before removing files. It
  also moves the remembered install folder from the vendor key of releases
  before the publisher was set (below).
- `src-tauri/installer/English.nsh` (`customLanguageFiles`): the
  installer's English text, Tauri's own with the data box's wording
  changed. It replaces Tauri's file whole: after a Tauri update, compare it
  with the `English.nsh` Tauri writes to `target/release/nsis/x64/`.
- `LICENSE` as the licence page and beside the program; the copyright line.

The publisher, who is also the copyright holder, is set once:
`bundle.publisher` in `src-tauri/tauri.conf.json`. It is the installed entry's
publisher, the programs' company name and the copyright line ("Copyright
(C) 2026 <publisher>", made by `release.mjs` and by `crates/cli/build.rs`).
Tauri also names the installer's vendor registry key after it
(`HKCU\Software\<publisher>\netterm`, where the install folder is
remembered): changing it moves that key. Releases up to 0.4.1 had no
publisher and used `HKCU\Software\netterm`; an upgrade from them takes the
folder over from there in its window and in passive mode, and removes the
old key. A silent upgrade (`/S`) of a copy installed elsewhere than
`%LOCALAPPDATA%\netterm` needs `/D=`.

## How the update check behaves

- Only builds made by the release script have the updater and a release
  server. Development builds (`app:dev`, `app:build`) check nothing.
- At start, if **Check for a new version at start** is on (default), the app
  fetches `latest.json` once (`netterm_core::update`). Off, it makes no
  connection on its own; **Check now** in the preferences still works.
- Installed copy: a notice offers **Install and restart**. Nothing is
  downloaded before that click (with open sessions it asks again). The
  Tauri updater then fetches the manifest, refuses anything but the version
  offered, downloads the installer, checks its signature (including the
  signed version, so an old release cannot be passed off as new) and runs
  it in passive mode; the app ends and the installer starts it again.
- A copy counts as installed only when the installer's `uninstall.exe` is
  beside `netterm.exe` and `portable.txt` is not. Every other copy (the
  portable zip, a program copied out of an installation, a development
  build) only announces the version and links to `page`; it never installs,
  so an update never adds a second installation.
- On Linux every copy (package or AppImage) only announces: only Windows
  has an installer the app updates itself with, so there the updater
  plugin is not even loaded.
- `NETTERM_UPDATE_ENDPOINT` set when the app runs overrides the release
  server (the smoke test points it at a local server). Plain http is
  accepted only for this computer, and a release build's updater refuses
  to install over http anyway.
