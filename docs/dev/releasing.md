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
Versions 0.1.0 to 0.5.0 were made this way and published on the tracker's
release page as pre-releases. The first signed release starts the updater:
copies of a pre-release cannot update themselves to it.

Steps 1, 2, 4 and 5 of "Each release" below apply as they are; step 3 is the
two commands above, and there is no `latest.json` to publish.

## What a release contains

`target/dist/<version>/`:

| File                                 | Purpose                                                                          |
| ------------------------------------ | -------------------------------------------------------------------------------- |
| `enodix_<version>_x64-setup.exe`    | NSIS installer, per user (no administrator rights); installs `enodix-cli.exe`, `LICENSE` and `THIRD-PARTY-NOTICES.txt` beside the app and the Python wheel in `python\` (see "The installer" below). |
| `enodix_<version>_x64-setup.exe.sig`| The updater signature of the installer (made with the release key).              |
| `enodix_<version>_x64-portable.zip` | `enodix.exe`, `enodix-cli.exe`, `python\enodix-<version>-py3-none-any.whl` (the Python bridge), `portable.txt` (makes the copy portable: it only notifies), README, `LICENSE`, `THIRD-PARTY-NOTICES.txt`.|
| `latest.json`                        | The update manifest the app checks (Tauri updater format, plus `page`).          |
| `enodix_<version>_amd64.deb`        | Debian/Ubuntu package: `/usr/bin/enodix`, `/usr/bin/enodix-cli`, the wheel in `/usr/lib/enodix/python/`. |
| `enodix-<version>-1.x86_64.rpm`     | Fedora/RHEL/openSUSE package, the same files.                                    |
| `enodix_<version>_amd64.AppImage`   | One file that runs without installing; `enodix-cli` and the wheel are inside it. |
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
   npx tauri signer generate -w "%USERPROFILE%\.enodix-release\enodix-updater.key"
   ```

   This writes `enodix-updater.key` (private, encrypted with the password)
   and `enodix-updater.key.pub` (public).

2. Keep the password in your password manager.

3. Back up the private key file **and** the password to two places that are
   not this computer, for example the password manager (as an attachment)
   and an encrypted USB stick kept elsewhere. Check that the backup opens.

4. Never commit the private key. `.gitignore` ignores `*.key`, and the
   release script refuses a key inside the repository.

5. Copy the public key into the repository and commit it:

   ```
   copy "%USERPROFILE%\.enodix-release\enodix-updater.key.pub" src-tauri\updater-key.pub
   ```

   The release script builds it into the app. (It can also be given with
   `ENODIX_UPDATER_PUBKEY`.)

6. Write down here where the key and its backups are (not the password):

   - Private key: created 2026-11-22 (key ID `9DF68927ED1EF3E2`, the public
     half is `src-tauri/updater-key.pub`); kept by the maintainer outside
     the repository, and as the repository secret
     `TAURI_SIGNING_PRIVATE_KEY` for the release workflow
   - Backups: with the password, in the maintainer's password manager

Also decide once **where `latest.json` is published**: one address that
never changes, because every installed copy keeps asking it. It must be
https with a certificate Windows trusts (the app uses the Windows
certificate store; a self-signed certificate on a private server fails the
check, harmlessly but always). On GitHub,
`https://github.com/<owner>/<repo>/releases/latest/download/latest.json`
always serves the newest release's file; check that your host offers an
equivalent, or publish the file at a fixed address yourself. enodix uses
that GitHub address: the release workflow below sets it from the
repository, so a release must be a normal one, not a pre-release (GitHub's
"latest" skips pre-releases).

For the release workflow, the key is also kept as two secrets of the
repository's `release` environment (Settings, Environments, release), which
only tags `v*` and the `main` branch may use: a workflow run from any other
branch cannot read them. Set them with `gh`, so that the password is pasted
and never written down:

```powershell
Get-Content -Raw "$env:USERPROFILE\.enodix-release\enodix-updater.key" | gh secret set TAURI_SIGNING_PRIVATE_KEY --env release --repo <owner>/enodix
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release --repo <owner>/enodix
```

The release script itself takes the key and its password out of its
environment before it builds anything, and gives them to one program only,
`tauri signer sign`, run on the finished installer: the build (every crate's
build script, the npm tools) never sees them.

## Releasing with GitHub Actions (the usual way)

`.github/workflows/release.yml` builds the Windows files and the Linux
packages on GitHub's runners with `npm run release`, signs the installer
for the updater with the secrets above, and publishes a GitHub Release with
every file, `latest.json` included, the version's section of
`CHANGELOG.md` and `SHA256SUMS.txt` as its notes. The release is always
published as a **pre-release**: GitHub's "latest" skips pre-releases, so
no installed copy is offered the update until you promote it (see "The
release gate"). It refuses a tag that does not match the version in
`Cargo.toml`, or a version the changelog has no section for, and, before
building anything, one that fails the gate's automatic checks.

1. On a branch, set the version and the changelog (step 1 of "Each release"
   below), open a pull request and merge it once CI passes.
2. On your computer, from the merged `main`: the checks CI cannot run, the
   smoke test (`npm run app:build && npm run smoke:hidden`), the containerlab
   suite (`npm run test:lab`) and the installer check in Windows Sandbox with
   the previous release (step 4 below, built with `npm run release -- --unsigned`).
3. If the workflow changed since the last release, try it first: Actions,
   Release, Run workflow, on `main`. It builds and signs every file and
   keeps them as the run's artifacts; it never publishes (only a tag does).
   The publish job takes from each platform only the files its build is
   meant to give, under their names, and publishes nothing if one is
   missing or anything else is there.
4. Make sure the gate's conditions hold ("The release gate" below), then tag
   the merged commit and push the tag; the workflow publishes a pre-release:

   ```
   git tag -s v<version> -m "<product> <version>"
   git push origin v<version>
   ```

5. Attach the evidence to the release's ticket and promote the pre-release
   (see "The release gate"). Then step 6 below (an installed copy of the
   previous version offers the update) and step 7 (clean up).
6. After promoting, run the Distribute workflow by hand (next section). Check
   that its run is green, and follow the winget pull request it opens until
   Microsoft's moderators merge it.

The steps below are the same release built and published from your own
computer, for when GitHub's runners are not available.

## The release gate

Decided on 2026-11-28 (ticket #76) after 0.8.0 shipped two regressions that
a smoke run would have caught. Nothing is tagged until all of this holds:

1. **Evidence on record.** On the release's ticket, attached or pasted: the
   green CI run on the commit to be tagged, the output of
   `npm run app:build && npm run smoke:hidden` on that commit, and the output
   of `npm run test:lab`. The installer checks of step 4 of "Each release"
   go there too.
2. **48 hours** between the last feature merge and the tag. A feature merge
   is any commit since the previous release that changes more than
   documentation, the changelog and the version lines of the manifests
   (`Cargo.toml`, `Cargo.lock`, `package.json`, `package-lock.json`,
   `tauri.conf.json`), and the issue templates, pull request template and
   `CODEOWNERS` under `.github/`. A dependency bump, a lock file change and
   any change to a workflow or action under `.github/` are feature merges.
   So the release's own version bump (which changes the issue template's
   version too) does not restart the wait, and can be tagged 48 hours after
   the last real change. Fixes found
   by the smoke or lab run are feature merges: they restart the wait.
3. **Pre-release first.** The workflow publishes every release as a
   pre-release (`--prerelease`, never `--latest`), so the updater and
   `releases/latest` do not see it. Promote it (edit the release, untick
   "pre-release", tick "latest") only after the evidence is attached and you
   have installed from its files. Then run Distribute.
4. **No tag mid-wave.** Tag only when the wave of tickets is finished and
   one full smoke run and one lab run have passed on that very commit. Any
   later merge, a fix included, means both are run again.
5. **Two settings of the repository**, which a workflow cannot make for you
   (see the ticket): a required reviewer on the `release` environment, and a
   tag ruleset on `v*` so that only the maintainer can create those tags.

What `.github/workflows/release.yml` enforces by itself, in its `gate` job
(`scripts/release-gate.sh`, which you can also run in a clone:
`scripts/release-gate.sh HEAD dry-run`; its cases are in
`npm run test:release-gate`), before the Windows and Linux builds
start and so before the `release` environment's secrets are used: CI
finished green on the tagged commit (`ci.yml` only: the Dependencies
workflow stays a manual check, step 2 of "Each release"), and 48 hours have
passed since the last feature merge, as defined above (computed from the
commit history, against the time the job runs, so a gate that failed
because the tag came too early passes on a re-run after the wait). It
refuses a shallow clone and a commit it cannot find. CI cancels a run on
`main` when a newer merge lands: a commit overtaken that way has no green
run, and the gate refuses it until CI is re-run on that commit. It also publishes only pre-releases, and no longer starts
Distribute. A run by hand (not a tag) prints the gate's failures as warnings
and goes on, since it publishes nothing. It cannot tell whether the smoke
and lab output exist, or whether the wave is over: those are yours.

### Review of every pull request before it merges

Each pull request gets this before a squash merge. The verdict goes in a
comment on the pull request.

- **A fresh reviewer**, given the issue and the diff and not the author's
  reasoning. The reviewer reads the area notes for the files touched and
  checks the standing rules of `CLAUDE.md`. Blocking findings go back to the
  implementer and are reviewed again after the fix. For a `security` issue the reviewer also writes a test of their
  own that tries to break the fix, and says what happened.
- **Mechanical checks from a clean clone**, after `rustup update stable`:
  `npm run format:check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `npm run lint`, and `npm run test:python` when
  `python/` or `crates/cli/` changed. CI must be green on Ubuntu and Windows.
- **For a bug**, revert the fix and see the new test fail; then restore it.
- **The device checks the change calls for**: the smoke test
  (`npm run app:build && npm run smoke:hidden`) for any change to `src/` or
  `src-tauri/`; the lab suite (`npm run test:lab`) for changes to
  `session/`, `job/`, `files/` or the platform definitions; the lrzsz test
  for modem code.
- **Docs in step with the change**: the area note in `docs/agents/areas/`,
  the user guide, `docs/guide/interface.md` when a feature moves, and the
  changelog.
- **Approval and green CI** before the squash merge.
- **A flaky test** is re-run on its own and the failure recorded in the
  pull request. It is never skipped, ignored or quarantined to get a merge.

## PyPI and winget

`.github/workflows/distribute.yml` puts a published release on PyPI (the
Python bridge's wheel, as `enodix`) and in winget (the installer, as
`ssosa-dev-net.enodix`; the ID never changes). The Release workflow does
not start it: you run it once the pre-release has been promoted, so that
nothing reaches PyPI or winget before the release gate is met. It builds nothing: it
downloads the release's portable zip and `SHA256SUMS.txt`, checks the one
against the other and takes the wheel out of the zip, so PyPI gets the same
bytes as the app ships. For winget, Komac (pinned, with its SHA-256) writes
the new version's manifest from the previous one; the job checks that the
manifest names the installer's SHA-256 from `SHA256SUMS.txt` and only then
opens the pull request on `microsoft/winget-pkgs`.

To run it (after promoting, a failed run, or a release made from your
computer):
Actions, Distribute, Run workflow, from `main`, with the version without
the `v`. Each half can be left out. Running it again is safe: PyPI skips a
version it already has, and Komac stops if a pull request for the version
is open.

PyPI refuses a wheel whose metadata it does not accept, most often a
classifier not on <https://pypi.org/classifiers/> (`scripts/build-wheel.mjs`
lists them). The wheel of 0.7.0 and earlier has one, so those versions are
not on PyPI.

### Once, before the first distribution

- **PyPI**: an account on pypi.org with two-factor authentication. Under
  Your account, Publishing, add a pending publisher: project `enodix`,
  owner `ssosa-dev-net`, repository `enodix`, workflow `distribute.yml`,
  environment `pypi`. The first upload creates the project. Until then the
  name is not reserved: someone else could still register it.
- **winget**: the first version is submitted by hand (Komac can only
  update a package that is already there): a pull request on
  `microsoft/winget-pkgs` with the three manifests of
  `manifests/s/ssosa-dev-net/enodix/<version>/`, from a fork of that
  repository under your account, which Komac then reuses. On the first pull
  request, Microsoft's bot asks you to accept its contributor licence
  agreement. Then a classic personal access token of the same account with
  the `public_repo` scope only, as the secret `WINGET_TOKEN` of the
  `winget` environment. It can push to every public repository of the
  account, enodix included: give it an expiry date, and replace it when it
  expires (the winget job then fails with a 401).

  ```powershell
  gh secret set WINGET_TOKEN --env winget --repo ssosa-dev-net/enodix
  ```

- **The environments** `pypi` and `winget` (Settings, Environments), each
  limited, like `release`, to the `main` branch and `v*` tags.

## Each release

1. Set the version in `Cargo.toml` (`[workspace.package] version`) and
   `package.json` (the script refuses if they differ). Versions are semver;
   the app offers only a higher version than its own. In `CHANGELOG.md`,
   rename "Unreleased" to the version and start a new "Unreleased" above it.
   Read `docs/trust.md` against the code of this version and correct what
   changed, since it says of itself that it was checked against the source
   code of the version it ships with.
2. `npm run check`, `npm run test:python` (the Python bridge: needs Python
   3.10+ and, the first time, the network for Nornir), and
   `npm run app:build && npm run smoke`. The Dependencies workflow must be
   green, and each advisory `deny.toml` ignores looked at again: drop the
   ones a dependency update has fixed (`cargo deny check` then reports an
   ignore that matches nothing).
3. Build the Windows files, in a shell where the key is set (PowerShell shown):

   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.enodix-release\enodix-updater.key"
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "<from the password manager>"
   $env:ENODIX_UPDATE_ENDPOINT = "https://<host>/<owner>/enodix/releases/latest/download/latest.json"
   $env:ENODIX_RELEASE_URL = "https://<host>/<owner>/enodix/releases/download/v0.2.0/"
   $env:ENODIX_RELEASE_PAGE = "https://<host>/<owner>/enodix/releases/tag/v0.2.0"   # optional
   $env:ENODIX_RELEASE_NOTES = "Serial break works on FTDI adapters."                 # optional
   npm run release
   ```

   `ENODIX_UPDATE_ENDPOINT` is built into the app: keep it the same for
   every release. `ENODIX_RELEASE_URL` is where this release's files will
   be downloadable; `latest.json` points the updater at the installer there.
   The first build downloads NSIS into Tauri's cache.

   Then the Linux packages, in the same shell (only
   `ENODIX_UPDATE_ENDPOINT` is used; no key). Build both from the same
   commit: the folder keeps the other platform's files, and a failed build
   leaves the other platform's files there.

   ```powershell
   npm run release:linux
   ```

   It copies the working tree into WSL (`~/enodix-linux-check`, see
   `docs/dev/linux.md` for the packages the distribution needs), runs
   `npm ci` and `npm run release` there and copies the .deb, .rpm and
   .AppImage into `target/dist/<version>/`. On a Linux machine, run
   `npm run release` there instead. The first build downloads the AppImage
   tools into Tauri's cache.

4. Check the installer, on a Windows computer where enodix is not
   installed for your user (the script stops otherwise), with the previous
   release's installer:

   ```
   npm run test:installer -- -Old <folder>\enodix_<previous>_x64-setup.exe
   ```

   `scripts/check-installer.ps1` installs the previous release, upgrades it
   to this one while the installed `enodix-cli.exe` runs, uninstalls
   (again with it running), installs this one fresh and uninstalls it, and
   upgrades a copy in another folder in passive mode, all silently or
   passively. After each step it lists the install folder, the uninstall
   entry, the vendor registry keys, the shortcuts and the version details,
   then puts the computer back. Check that each upgrade leaves one wheel,
   `LICENSE`, `THIRD-PARTY-NOTICES.txt` and the new `enodix-cli.exe`, that the running one was
   closed (Windows takes about 30 seconds to end it), that each uninstall
   leaves no install folder, entry or shortcut, and that the last lines say
   the data folders are as they were. It never
   starts the app and never ticks the data box: the wizard's pages, the box
   and the updater's own install are not exercised. Paste its output into
   the release's ticket.

   Then the install-and-run check, on a clean Windows: in Windows Sandbox
   (the feature turned on once), with
   nothing of yours at risk:

   ```
   npm run test:installer:sandbox -- --old <folder>\enodix_<previous>_x64-setup.exe
   ```

   It runs the check above inside the sandbox. Then it installs the
   previous release, starts it once, puts config backups, job results and
   a Python environment where versions up to 0.4.1 kept them (the roaming
   data folder), installs this release over it and starts it: every file
   must then be in the local data folder, the same bytes, and gone from
   the roaming one, the settings must still be there, and a second start
   must change nothing (#128). Then it installs this release,
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

5. Publish by hand: create the release (tag `v<version>`) **as a
   pre-release**, promoted only as "The release gate" says, upload every file
   from `target/dist/<version>/`, and paste the version's section of
   `CHANGELOG.md` and `SHA256SUMS.txt` into the release notes. Make `latest.json` reachable at `ENODIX_UPDATE_ENDPOINT` **last**,
   once the installer can be downloaded.

6. Check: an installed copy of the previous version shows the notice
   (**Check now** in Preferences), and installing works. On Linux, install
   the .deb in WSL and run `npm run test:linux-desktop` (also with
   `--backend x11` and `--app <the AppImage>`), then check what it cannot
   on a real desktop with the list in `docs/dev/linux.md`.

7. Clean up, so build output does not pile up (it reached hundreds of
   gigabytes once). Everything published can be downloaded again:
   - in `target/dist/`, keep only the folder of the release just
     published (the next release's installer check needs its
     `setup.exe` as the previous version) and remove the older ones
     and the `stage-*` folders;
   - remove `target/debug`, `target/release` and `target/sandbox`;
   - in WSL, remove `~/enodix-linux-check/target` and the packages
     the Linux build left there.

The release build replaces `target/release/enodix.exe` with one that
has the updater; run `npm run app:build` again before the smoke test.

## The installer

Tauri generates the NSIS installer; `scripts/release.mjs` adds to it:

- `src-tauri/installer/hooks.nsh` (Tauri's `installerHooks`): a running
  `enodix-cli.exe` is closed like the app before installing or
  uninstalling; installing removes the wheels of other versions from
  `python\`, uninstalling every wheel there, then `python\` and the program
  folder if empty; with "Delete the application data" ticked, the
  uninstaller runs `enodix-cli clear-keychain --yes` before removing files. It
  also moves the remembered install folder from the vendor key of releases
  before the publisher was set (below).
- `src-tauri/installer/English.nsh` (`customLanguageFiles`): the
  installer's English text, Tauri's own with the data box's wording
  changed. It replaces Tauri's file whole: after a Tauri update, compare it
  with the `English.nsh` Tauri writes to `target/release/nsis/x64/`.
- `LICENSE` as the licence page and beside the program; the copyright line.
- `THIRD-PARTY-NOTICES.txt` beside the program: every component of other people's the
  build links or bundles, its licence and the licence texts
  (`scripts/third-party-notices.mjs`, run by the release for each platform;
  it fails the release if a component has no licence text).

The publisher, who is also the copyright holder, is set once:
`bundle.publisher` in `src-tauri/tauri.conf.json`. It is the installed entry's
publisher, the programs' company name and the copyright line ("Copyright
(C) 2026 <publisher>", made by `release.mjs` and by `crates/cli/build.rs`).
Tauri also names the installer's vendor registry key after it
(`HKCU\Software\<publisher>\enodix`, where the install folder is
remembered): changing it moves that key. Releases up to 0.4.1 had no
publisher and used `HKCU\Software\enodix`; an upgrade from them takes the
folder over from there in its window and in passive mode, and removes the
old key. A silent upgrade (`/S`) of a copy installed elsewhere than
`%LOCALAPPDATA%\enodix` needs `/D=`.

## How the update check behaves

- Only builds made by the release script have the updater and a release
  server. Development builds (`app:dev`, `app:build`) check nothing.
- At start, if **Check for a new version at start** is on (default), the app
  fetches `latest.json` once (`enodix_core::update`). Off, it makes no
  connection on its own; **Check now** in the preferences still works.
- Installed copy: a notice offers **Install and restart**. Nothing is
  downloaded before that click (with open sessions it asks again). The
  Tauri updater then fetches the manifest, refuses anything but the version
  offered, downloads the installer, checks its signature (including the
  signed version, so an old release cannot be passed off as new) and runs
  it in passive mode; the app ends and the installer starts it again.
- A copy counts as installed only when the installer's `uninstall.exe` is
  beside `enodix.exe` and `portable.txt` is not. Every other copy (the
  portable zip, a program copied out of an installation, a development
  build) only announces the version and links to `page`; it never installs,
  so an update never adds a second installation.
- On Linux every copy (package or AppImage) only announces: only Windows
  has an installer the app updates itself with, so there the updater
  plugin is not even loaded.
- `ENODIX_UPDATE_ENDPOINT` set when the app runs overrides the release
  server (the smoke test points it at a local server). Plain http is
  accepted only for this computer, and a release build's updater refuses
  to install over http anyway.
