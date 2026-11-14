# Making a release

A release is built on Windows with one command, `npm run release`
(`scripts/release.mjs`). It builds and checks the files; it never uploads,
tags or publishes anything. Publishing is done by hand, as below.

## What a release contains

`target/dist/<version>/`:

| File                                 | Purpose                                                                          |
| ------------------------------------ | -------------------------------------------------------------------------------- |
| `netterm_<version>_x64-setup.exe`    | NSIS installer, per user (no administrator rights).                              |
| `netterm_<version>_x64-setup.exe.sig`| The updater signature of the installer (made with the release key).              |
| `netterm_<version>_x64-portable.zip` | `netterm.exe`, `portable.txt` (makes the copy portable: it only notifies), README.|
| `latest.json`                        | The update manifest the app checks (Tauri updater format, plus `page`).          |
| `SHA256SUMS.txt`                     | SHA-256 of every other file, in `sha256sum` format.                              |

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
   the app offers only a higher version than its own.
2. `npm run check`, and `npm run app:build && npm run smoke`.
3. Build, in a shell where the key is set (PowerShell shown):

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

4. Publish by hand: create the release (tag `v<version>`), upload every file
   from `target/dist/<version>/`, and paste `SHA256SUMS.txt` into the release
   notes. Make `latest.json` reachable at `NETTERM_UPDATE_ENDPOINT` **last**,
   once the installer can be downloaded.

5. Check: an installed copy of the previous version shows the notice
   (Preferences → **Check now**), and installing works.

Note: the release build replaces `target/release/netterm.exe` with one that
has the updater; run `npm run app:build` again before the smoke test.

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
- `NETTERM_UPDATE_ENDPOINT` set when the app runs overrides the release
  server (the smoke test points it at a local server). Plain http is
  accepted only for this computer, and a release build's updater refuses
  to install over http anyway.
