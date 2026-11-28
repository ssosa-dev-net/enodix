# Release and Linux

Packaging, the update check, the Linux checks and the Linux release.

- Packaging and update check (#28, `src/update.rs`, `scripts/release.mjs`,
  `docs/dev/releasing.md`, `README.md`). `UpdateChecker::new(Option<UpdateSource>,
current_version, InstallKind)`: `automatic(&prefs)` → `Disabled` (no
  connection) when `Preferences.check_for_updates` is off, else
  `check_now()` → `UpdateStatus::NotConfigured | UpToDate | Available(
UpdateOffer { version, current, notes, published, page, action:
Install|Notify }) | Failed { reason }` (serde `kind`, camelCase). One GET
  of `latest.json` (Tauri static format + our `page`), reqwest 0.13 +
  rustls `ring` (same stack as tauri-plugin-updater; `rustls-platform-verifier`
  = Windows store), 15 s timeout, 1 MiB cap; semver compare (leading `v`
  ok). `UpdateSource` = https, or http to loopback/localhost only.
  `InstallKind::detect(exe dir)` = `Installed` only if NSIS's
  `uninstall.exe` (`INSTALLED_MARKER`) is beside the exe and `portable.txt`
  (`PORTABLE_MARKER`) is not; anything else (zip, copied exe, dev build)
  is `Portable` → `Notify` only. The checker keeps the last offer
  (`offer()`); `installable(updater_present)` → offer or `InstallRefused::
NothingOffered | NotifyOnly | NoUpdater`; `UpdateOffer::is_version`
  (semver); `UpdateSource::choose(override, built_in)`. Shell: the
  updater plugin is registered **only** when the config has
  `plugins.updater` (the release script passes it with `--config`;
  without a pubkey the plugin would fail to start); `Updates` state;
  commands `update_build`, `update_check_automatic`, `update_check_now`,
  `update_install` (`installable`, the plugin asks the checker's source,
  `is_version` its answer; then `download_and_install`, which exits on
  Windows), `update_open_page` (through `open_in_browser`,
  shared with web links and their `ENODIX_WEB_LINKS_TO` seam).
  `ENODIX_UPDATE_ENDPOINT` overrides the server (smoke seam). Interface:
  `updates.svelte.ts` (`startUpdates` in `App.svelte`),
  `UpdateNotice.svelte` (fixed bottom-right; `[data-update=install|notify]`,
  `data-update-version`, `[data-update-install|later|page|error|confirm]`;
  asks again when sessions are live), Preferences
  `preference-check-for-updates`, `[data-update-check-now]`,
  `[data-update-status=<kind>]`. Smoke: `updateCheck` (start) and
  `portableUpdateCheck` (after the restart); it writes an empty
  `target/release/uninstall.exe` for the first launch, `portable.txt` for
  the restart, and removes both. Release: NSIS only (no
  WiX), `currentUser`, `createUpdaterArtifacts`, `requireSignedVersion`;
  the script verifies the `.sig` against the public key (minisign in
  node crypto), zips with Windows' `System32\tar.exe` (Git's tar cannot
  zip), writes `SHA256SUMS.txt`, uploads nothing. The release build
  overwrites `target/release/enodix.exe`: rerun `app:build` before the
  smoke. No real key exists yet (`src-tauri/updater-key.pub` is the
  user's to create). Not verified: the updater's own install (no signed
  release yet).
- Installer (#111, `src-tauri/installer/hooks.nsh` and `English.nsh`,
  `scripts/check-installer.ps1`, `docs/dev/releasing.md` "The installer").
  `release.mjs` writes `target/installer-hooks.nsh` (`!define ENODIX_*`:
  product, publisher, description, old vendor = identifier's 2nd part, cli,
  wheel; then `!include` of `hooks.nsh`) for `nsis.installerHooks`, and
  passes `customLanguageFiles.English` (a full copy of Tauri 2.12's
  English.nsh, `deleteAppData` reworded, plus `enodixClearKeychain` /
  `enodixKeychainNotCleared`: recompare on a Tauri bump), `licenseFile` and
  `LICENSE` as a resource, `copyright` ("Copyright (C) 2026 <publisher>").
  Third-party notices (2026-11-22): `scripts/third-party-notices.mjs`
  (`writeThirdPartyNotices`) writes `target/release/THIRD-PARTY-NOTICES.txt` for
  the platform being built: the normal, non-proc-macro dependencies of
  `enodix` and `enodix-cli` from `cargo metadata --filter-platform`, the Rust
  standard library, and the non-dev npm packages of package-lock.json, each
  with its own licence files (else a sibling crate's from the same
  repository, else the standard text from the toolchain's
  `share/doc/rust/licenses`, MPL-2.0 from another crate). It fails if any
  component has no text. `release.mjs` runs it in `windows()` and `linux()`;
  it goes beside the program (installer resource, portable zip) and at
  `LINUX_NOTICES` (beside `LINUX_LICENCE`), which `checkLinuxPackage`
  checks. 378 components for Windows, 451 for Linux, about 0.9 MB.
  RPM post-removal (2026-11-22): `rpm -e` left `/usr/lib/<product>/python` and
  `/usr/share/licenses/<product>` empty (the .rpm records files, not all
  folders); `linux()` writes `target/release/rpm-post-remove.sh` (on `$1 = 0`
  only, `rmdir` of those folders and `/usr/lib/<product>`, each `|| :`) and
  passes it as `bundle.linux.rpm.postRemoveScript`. Checked in Ubuntu 24.04
  and Fedora 41 podman containers in WSL (`--network host`: rootless podman
  there has no pasta): install, reinstall over itself (all files kept), remove,
  then `find / -iname "*enodix*"` empty; the .deb needed nothing.
  Tauri includes the hooks file before its defines and `Var`s: only macros
  use them; the top level has `VIAddVersionKey` CompanyName/Comments (the
  template sets the others, a second key is an error) and
  `MUI_CUSTOMFUNCTION_GUIINIT` (MUI's `.onGUIInit`, not run with `/S`).
  Hooks: PREINSTALL `CheckIfAppIsRunning` on `enodix-cli.exe` (Tauri's
  Restart Manager macro; inserted three times in Section Uninstall without
  label clashes); POSTINSTALL deletes `python\enodix-*-py3-none-any.whl`
  but this version's, writes the entry's `Comments`, deletes
  `HKCU\Software\enodix\enodix` (+ parent if empty) when the publisher
  differs; PREUNINSTALL checks the app then the cli, then, if
  `$DeleteAppDataCheckboxState = 1` and not `$UpdateMode`, `nsExec::ExecToLog
  enodix-cli clear-keychain --yes` (#127; failure: DetailPrint + MessageBox, the
  uninstall goes on); POSTUNINSTALL deletes every such wheel, `RMDir`
  python and `$INSTDIR`. GUIINIT copies the old vendor key's folder to the
  new key and to `$INSTDIR` (only if `$INSTDIR` is the default) when the
  new key is missing. Publisher: `bundle.publisher` in tauri.conf.json is
  Tauri's MANUFACTURER, so the vendor key is `HKCU\Software\Sergio
  Sosa\enodix` (Tauri's data box deletes it; a plain uninstall keeps it).
  `enodix.exe`'s FileDescription stays the product name (tauri-build
  sets it; `shortDescription` reaches no Windows file); the installer's
  likewise (template). Check script: `npm run test:installer -- -Old
  <setup.exe>`; the running cli is `enodix-cli tunnel <empty ws>` with
  its own `ENODIX_DATA_DIR`, ended by the installer's Restart Manager
  after about 30 s (a console program); after the fresh install,
  `CheckClearKeychain` runs the installed `clear-keychain` without and with
  `--yes` on the prefix `enodix-test-installer-check-<pid>` and reports
  both (#127; the script was not run since); NSIS 3.13's uninstaller copy is
  `%TEMP%\~nsuN.tmp\Un.exe` (older: `Un_A.exe`). The smoke's empty
  `target/release/uninstall.exe` and `InstallKind::detect` are untouched.
  Verified (2026-11-21, 0.4.0 to this build): one wheel after the upgrade,
  the running cli closed and replaced, no folder after each uninstall, the
  passive upgrade of a `/D=` copy going to its folder, the metadata, the
  old key gone, data folders unchanged; the uninstall page's two-line box
  seen in a picture (window ended, nothing clicked). Not exercised: the
  wizard's pages (licence, "Uninstall before installing"), the data box and
  so the uninstaller's call of `clear-keychain` (read in the generated
  `installer.nsi`; the command has its own tests), the updater's install.
  Since #139 the data box is exercised in Windows Sandbox (`npm run
  test:installer:sandbox`, `scripts/sandbox-installer.ps1`, see
  `testing.md`): the uninstaller's window driven with UI Automation, the
  box ticked with `BM_SETCHECK` (it is a plain BUTTON Tauri's page makes
  with `CreateWindowEx`), Uninstall and Close clicked with a posted
  `BM_CLICK`. There the published 0.4.1's `enodix-cli.exe` did not start
  (no VC++ runtime on a clean Windows; fixed since by linking it
  statically), so its "running cli closed by the installer" line meant a
  program that had already ended.
- Linux (#56, `scripts/test-linux.mjs`, `docs/linux.md`). Everything
  below the window passes on Linux (Ubuntu 26.04 in WSL 2): clippy
  `-D warnings` and the tests of `enodix-core` and `enodix-cli`, and
  `npm run test:python`; `src-tauri` is not built there (WebKitGTK).
  `npm run test:linux` runs those in place on Linux, or on Windows
  rsyncs the tree (`.gitignore` filter, no `.git`, no `NUL`) into WSL's
  `~/enodix-linux-check` (`ENODIX_WSL_DIR`, `ENODIX_WSL_DISTRO`) and
  runs them there (never build on `/mnt`). Core fixes: `python::
quiet_command` starts the child with `setsid` on Unix (no controlling
  terminal, like `CREATE_NO_WINDOW`, so the cli never prompts on a tty
  nobody sees; the one process group) and `python::stop` kills the
  group (`libc`, a Unix-only dependency; a grandchild that calls
  `setsid` itself escapes it; since #124 a keeper process in the group
  ends it when the script's `Tree` is dropped or the app ends, so a
  script that exits normally no longer leaves what it started running:
  `python-scripts.md`, "Ending with the app"; both now live in
  `process::start` / `Tree::stop`); `python::spawn` (now in
  `process::start`) retries
  `ExecutableFileBusy` for 2 s (ETXTBSY: a script just written while
  another thread forks; the tests' stand-in interpreters hit it); a
  script job's stdout and stderr share one pipe, so lines keep their
  order; the watcher adds the contents of a created folder to the batch
  (inotify watches a new folder only once reported, missing files put
  in it before); `EntryKind` dead code off Windows allowed. Skipped by
  platform: keychain tests, the 240.0.0.0/4 test, Pageant, the pipe DACL
  and its refused client, the cli's `no_desktop.rs`,
  the undeletable vault file, Python's default cli location. Known
  gaps: the serial driver on real ports and macOS are not checked (the
  app and packages: #57).
- Linux release (#57, `vault.rs`, `update.rs`, `scripts/release.mjs`,
  `scripts/release-linux.mjs`, `scripts/wsl.mjs`, `docs/linux.md`).
  No Linux keychain: `KeychainStore::is_supported()` (= Windows);
  `ActiveStore::without_keychain(vault_file)` keeps credentials only in
  the vault (a private `NoKeychain` store: reads find nothing, puts fail
  "...turn on the vault to store..."); `has_keychain()`,
  `needs_vault()` (no keychain and `Off`); `turn_off` → `VaultError::
NoKeychain` ("no other store", checked before the password);
  `start_over` → `Off` again. The cli, app closed and no vault, refuses
  "keeps them only in its vault, which is not turned on". Updates:
  `InstallKind::detect` is `Portable` off Windows (notify only); the
  shell registers the updater plugin only on Windows but still reads the
  check's address from `plugins.updater.endpoints`. Shell: store chosen
  by `is_supported()` (`ENODIX_CREDENTIAL_STORE=none` = without
  keychain: the smoke's seam), `vault_keychain` → bool,
  `ProfilesView.needs_vault`, `VaultFailure` kind `noKeychain`; the wheel
  folder is `resource_dir()/python` (exe dir on Windows and in
  `target/`, `/usr/lib/<product>` in a package, `$APPDIR/usr/lib/...` in
  the AppImage), the cli still beside the exe. Interface:
  `TurnOnVaultForm.svelte` (the `form[data-vault-form]` of VaultSection,
  now shared); `VaultSection` `[data-vault-keychain]`,
  `[data-vault-no-keychain]`, turn-off without keychain shows
  `[data-vault-off-no-keychain]` (no form); `ProfileForm` with
  `needs_vault` shows `[data-needs-vault=explain|turn-on]`
  (`[data-needs-vault-action=back|turn-on]`), then saves the typed
  profile once the vault is on; the Credentials view (#66)
  `[data-profiles-needs-vault]`. Release: `release.mjs` on Linux builds deb/rpm/AppImage
  (`bundle.linux.*.files` puts `enodix-cli` in `/usr/bin` and, since
  #127, `LICENSE` at `LINUX_LICENCE`: `.deb` `/usr/share/doc/<product>/
copyright`, `.rpm` and AppImage `/usr/share/licenses/<product>/LICENSE`;
  the wheel as a resource; `plugins.updater` = endpoint only; no key, no
  signature, not in `latest.json`); `checkLinuxPackage` then lists each
  package (`dpkg-deb --contents`, `rpm2cpio | cpio --list`, the AppImage's
  `--appimage-offset` + `unsquashfs -l -o`) and fails the release unless it
  holds the cli, the wheel and the licence (tools missing → fail naming
  them; `docs/linux.md` lists `rpm2cpio cpio squashfs-tools`); each platform clears only its own files in
  `target/dist/<v>/`, `release-files.mjs` `writeChecksums` lists all.
  `npm run release:linux` (Windows) syncs into WSL (`wsl.mjs`, shared
  with `test:linux`), `npm ci`, `release.mjs`, copies the packages back.
  Tests: `tests/vault.rs` (`without_a_keychain_*`), `tests/system_agent.rs`
  (`SshAgent::System` = `SSH_AUTH_SOCK`, the binary's one test),
  `update_check.rs` (off-Windows notify), cli `with_the_app_closed_and_no_
vault_*` (Unix). Smoke: `noKeychain` (after `vaultUpkeep`, restarts with
  `none`). Not verified: the packages on a real desktop (#58,
  `docs/linux.md` has the check list), real serial.
- The app under WSLg (#114, `scripts/linux-desktop-check.mjs`,
  `npm run test:linux-desktop [-- --app <program>] [--backend wayland|x11]
[--pictures <folder>]`, `docs/linux.md` "The app under WSLg" with the
  per-point table). On Windows it syncs into WSL like `test:linux`
  (`ENODIX_WSL_DIR`); on Linux it runs in place. WebDriver over
  `tauri-driver` (cargo) + `WebKitWebDriver` (apt `webkitgtk-webdriver`;
  tauri-driver sets `TAURI_WEBVIEW_AUTOMATION`, which a release build
  honours), a fetch-only client in the script (no npm dependency):
  Element Click/Clear/Send Keys, Actions (double click, key chords;
  WebDriver key codes built with `String.fromCodePoint`), Execute Script
  (sync/async; window commands as `__TAURI_INTERNALS__.invoke(
"plugin:window|…")`, where `unmaximize`/`unminimize` are refused by the
  ACL), Take Screenshot (the web view only; under X11 also ImageMagick's
  `import -window <xdotool id>`, as WSLg's XWayland cannot capture its
  root). Ports 24722 ssh, 24723 telnet, 24724 release server, 24727
  tauri-driver, 24728 WebKitWebDriver. The app inherits tauri-driver's
  environment: `ENODIX_DATA_DIR`, `XDG_DATA_HOME`/`CACHE`/`CONFIG` (the
  web view's own data otherwise goes to `~/.local/share/<identifier>`
  whatever `ENODIX_DATA_DIR` says), `SSH_AUTH_SOCK` (its own `ssh-agent
-D -a` holding `tests/fixtures/keys/rsa_client`),
  `ENODIX_UPDATE_ENDPOINT`, a `bin/` first on PATH with recording
  `xdg-open`, `gio`, `gnome-open`, `kde-open` and `powershell.exe` (the
  `open` crate tries `powershell.exe` first under WSL, the target in
  `OPEN_RS_TARGET`, a folder as `\\wsl$\<distro>\…`). Deleting the
  WebDriver session leaves the app running: `quit` kills the driver's
  process tree. The multi-send check reads the first session after
  Ctrl+PageUp (a hidden tab holds its output). Selectors are scoped to
  the session's `.slot[data-tab=…]`: with two panes there are two
  `.slot[data-active]`. Fixes: `update::InstallKind::Package`
  (`PACKAGE_MARKER` `package.txt` in the resource folder;
  `detect(program, resources)`, the portable marker wins; notify only);
  `release.mjs` writes `target/release/package.txt` into the .deb's and
  .rpm's `/usr/lib/<product>/` (not the AppImage's); the notice says
  "update it with the system's package manager" (`updates.build.kind`), a
  portable copy's "put it in place of this one", and Preferences' version
  line says which; `bundle.category: "Utility"` (the .desktop had an
  empty `Categories=`); `python::quiet_command` drops what points into
  `APPDIR` (`BUNDLE_VARIABLE`, `leave_the_bundle`; a list keeps its other
  entries): the AppImage found no Python (`PYTHONHOME`). Tests:
  `update_check.rs` (`a_copy_installed_by_a_package_*`,
  `a_copy_whose_resources_carry_the_package_marker_is_a_package`),
  `tests/bundle_environment.rs` (a binary of its own: it sets the
  process's environment). Verified 2026-11-21 (WSLg 1.0.79, Ubuntu
  26.04.1, WebKitGTK 2.52.6): the .deb on Wayland and X11, the AppImage on
  Wayland, every check passed; llvmpipe (the user is not in `render`),
  WebGL on; nothing like `WEBKIT_DISABLE_DMABUF_RENDERER` needed. Not
  checkable under WSLg: window drag, edge resize, Minimize (WSLg's
  compositor owns placement; `xdotool` moves only XWayland's pointer, not
  the compositor's grab), edge tiling, the app menu, serial, a real file
  manager or browser. Under WSLg the clipboard is Windows'.
- A desktop out of sight (#58, `scripts/linux-desktop-session.mjs`,
  `scripts/linux-desktop-input.py`, `scripts/linux-fedora-check.mjs`,
  `scripts/linux-fedora/Containerfile`; `docs/linux.md` "A desktop out of
  sight" has the per-item table). `npm run test:linux-desktop -- --desktop
xfce|gnome [--app <AppImage>] [--gallium-driver d3d12]` runs the whole
  check inside a desktop session of its own: `startDesktop(kind, folder)`
  gives an env with its own `XDG_RUNTIME_DIR`/`CONFIG`/`DATA`/`CACHE`
  (WSLg's sockets unreachable, `PULSE_SERVER` dropped) and D-Bus bus
  (`dbus-daemon --print-address`), helpers `windows(classes)` (the window
  manager's view: X11 `_NET_CLIENT_LIST` + `WM_CLASS` + xdotool geometry +
  `_NET_WM_STATE`; GNOME `Eval` over `meta_window`s, `app` from
  `Shell.WindowTracker`), `workArea`, `to/down/up/key/type`, `activate`,
  `minimize`, `place`, `picture`, `setDark`, `searchMenu`/`launchMenuHit`,
  `tileLeftKey`, `stop` (also SIGKILLs every process whose environ names
  the session's runtime folder: the file manager and browser openers
  started). xfce: `Xvfb :N -displayfd 1` (WSL's `/tmp/.X11-unix` is
  WSLg's, 0777 not 1777: Xvfb listens on the abstract socket only;
  `freeDisplay` reads `/proc/net/unix`), xfsettingsd, xfwm4 (seeded
  `xfwm4.xml`: `wrap_windows` false, else dragging to the edge wraps to
  the next workspace and tiles nothing, Thunar neither), xfce4-panel
  (seeded `xfce4-panel.xml`: whiskermenu, tasklist, clock); input xdotool
  (XTEST); dark = `xfconf-query xsettings /Net/ThemeName Adwaita-dark`.
  xdotool's geometry is off by the frame for decorated windows (Thunar),
  right for the app (no frame). gnome: `gnome-shell --headless --wayland
--no-x11 --unsafe-mode --virtual-monitor 1600x1000 --wayland-display
enodix-check-wayland`, animations and hot corners off (dconf in the
  session's config folder); wait for `!Main.layoutManager._startingUp`
  before hiding the start's overview, then one real click on Activities
  and a hide: hidden by Eval during start-up, the shell hands no pointer
  button to any window (clicks and drags silently do nothing). Input:
  `linux-desktop-input.py` (python3-gi) keeps a Mutter RemoteDesktop
  session (it lives as long as its D-Bus connection: gdbus calls lose it)
  and moves with `NotifyPointerMotionRelative` from `global.get_pointer()`
  (absolute needs a ScreenCast stream, i.e. PipeWire); Clutter's virtual
  device from Eval moves the pointer but its buttons reach nothing.
  Pictures: `org.gnome.Shell.Screenshot`. Dark = gsettings `color-scheme`.
  The check with a desktop: `desktopMenu` (desktop-file-validate, GTK
  icon lookups 16-256 via python3-gi, the hicolor cache, the menu's
  search started it; GNOME: tied to `enodix.desktop`), `desktopTheme`
  (the page's `prefers-color-scheme`, `data-theme` and change events),
  `frame` (WebDriver double click skipped: it leaves xfwm4's state
  maximized at the normal size), `desktopFrame` (`pageOrigin` from a
  `mousemove` listener; drag the bar, six edges/corners, tile by drag and
  by key, drag off, real double click; then `place` back), real opener
  (`xdg-open` stub records argv and env, unsets `WEBKIT_INSPECTOR_SERVER`
  and `TAURI_*AUTOMATION` (GNOME Web, being WebKit, fails to start with
  them), then execs `/usr/bin/xdg-open`; `powershell.exe` stub exits 1 so
  the `open` crate falls through as off WSL; `mimeapps.list` and XFCE's
  `helpers.rc` name Thunar and GNOME Web), `desktopOpener` (windows,
  release server visits with the browser's User-Agent, `/proc/<pid>/environ`
  and `cwd` of the opened programs), `desktopMinimize`. Release page and
  web link are now `http://127.0.0.1:24724/...` (served, recorded).
  Without `--desktop`, also new: the serial console (`socat
PTY,link=<temp>/ttyCONSOLE TCP-LISTEN:24729`, the device played over
  TCP; Send break from `[data-session-menu]`; the close confirmation
  `[data-confirm-close] [data-confirm=close]`), `paneTool` (a narrow
  pane's tools fold into `[data-pane-tools-more]`, #100: entries found by
  text), and webkit://gpu read (WebDriver navigates there and back:
  WebGL's debug renderer says "Apple GPU" in WebKit). Fedora:
  `npm run test:linux-fedora [-- --rpm <f>] [--desktop xfce|gnome]
[--fedora 45]` builds `enodix-fedora-check` (dnf installs the .rpm
  with python3, xdg-utils, thunar, epiphany, desktop-file-utils), checks
  the entry and icons in the image, and runs the desktop check with
  `--app`/`--cli` wrappers (`podman run --rm -i --network host
--userns=keep-id --env-host -e HOME=<temp> -v /tmp:/tmp -v
/dev/pts:/dev/pts`) and `--packaged rpm`; WebDriver stays on the host.
  Product fixes: `session/serial.rs` raises RTS best effort (a pseudo
  terminal refused it with ENOTTY, so a console forwarded with `socat
pty` could not be opened; `tests/serial_pty.rs`, Unix: bytes both ways,
  speed/stop bits/IXON held by the kernel (a pty keeps CS8, no parity),
  break accepted and ordered, in use refused, the other end closing →
  `PortLost`); `opener.rs` (`in_a_bundle`, `open_detached`: the `open`
  crate's commands, each through `leave_the_bundle`, without
  `APPDIR`/`APPIMAGE`/`ARGV0`/`OWD`, in `$HOME`, setsid, 250 ms failure
  check, then the next) used by `logs_open_folder` and `open_in_browser`
  from an AppImage (`tests/bundle_opener.rs`); `release.mjs` creates
  `target/release` before writing `package.txt` (a fresh tree failed).
- The AppImage's dark style (#143, `src-tauri/appimage/desktop-style.sh`,
  `release.mjs` `addStyleHook`, `APPIMAGE_STYLE_HOOK`). linuxdeploy's GTK
  hook (`apprun-hooks/linuxdeploy-plugin-gtk.sh`, written by the plugin
  Tauri downloads to `~/.cache/tauri`) pins `GTK_THEME=Adwaita:<variant>`
  from the portal's `color-scheme`, else gsettings'; XFCE's dark is a
  style, not a colour scheme, so it was ignored. The pin is kept. enodix's
  hook, sourced right after it, reads the style once (XFCE in
  `XDG_CURRENT_DESKTOP`: `xfconf-query -c xsettings -p /Net/ThemeName`;
  else gsettings `org.gnome.desktop.interface gtk-theme`; both with
  `GSETTINGS_SCHEMA_DIR`/`GIO_MODULE_DIR` unset, `timeout 2`); a name
  matching `*-dark` or `*-dark-*` (any case) sets `APPIMAGE_GTK_THEME` and
  `GTK_THEME` to `Adwaita:dark`. An exported `APPIMAGE_GTK_THEME` (the
  user's) is left alone (`printenv`: the GTK hook's own assignment is not
  exported). POSIX sh, safe under AppRun's `set -e`. Tauri's
  `appimage.files` cannot add it: they land only under the AppDir's `usr/`
  (an `/apprun-hooks/...` entry vanished). So `addStyleHook` unpacks the
  built AppImage (`unsquashfs -o <--appimage-offset>`), copies the hook,
  inserts `source "$this_dir"/apprun-hooks/"enodix-desktop-style.sh"`
  after the GTK hook's line in `AppRun` (fails if that line is missing),
  `mksquashfs -all-root -noappend -comp <same> -b <same>` and writes the
  original runtime bytes plus the new squashfs. `checkLinuxPackage` requires
  `APPIMAGE_STYLE_HOOK` in the AppImage. Check: the desktop's `darkBy`
  (`style` xfce, `colour scheme` gnome); for a portable `--app` on a style
  desktop `pinnedTheme` requires light at launch and only notes the live
  switch; `restart` with a desktop now makes the desktop itself dark
  (`setDark(true)`, no `GTK_THEME` in the env) and requires the app dark
  at launch, for every package. Verified 2026-11-22: the AppImage built
  without the hook fails that step on XFCE (`applied: light`), with it
  passes. Not verified: other desktops whose dark is a style (MATE,
  Cinnamon, KDE's GTK settings), a style named otherwise (`Nordic`, ...).
- The data folders (#128, 0.5.0; `crates/core/src/data_folders.rs`). On
  Windows the app has a settings folder (`app_data_dir`, roaming
  `%APPDATA%\dev.enodix.app`: preferences, `state.yaml`, the vault and
  its lock, `known_hosts`, `python.yaml`, the agent's endpoint) and a
  local one (`app_local_data_dir`, `%LOCALAPPDATA%\dev.enodix.app`:
  `logs` (always was), `configs`, `jobs`, `python`, WebView2's
  `EBWebView`). Elsewhere, and with `ENODIX_DATA_DIR`, one folder
  (`DataFolders::in_one`): Linux and macOS do not change. Shell `setup`:
  `data_folders::settle(settings, local)` first, before any store is
  made; `RunStore`, `ConfigStore` and `PythonSetup` (now given the
  `DataFolders`) use `settled.folders`; `DataMove(Settled)` is managed
  (it holds the lock for the app's life); command `data_notices` →
  `[Notice { what, kind, message }]`; `DataNotice.svelte` (App, top right,
  `[data-data-notice=n]`, items `[data-data-notice-item=<kind>]
[data-data-notice-what]`, `[data-data-notice-dismiss]`) shows them; the
  same go to stderr with what moved. The move, per `MOVED` (`configs`,
  `jobs`, `python`), only for one the local folder lacks (or holds as an
  empty folder): `rename`; on `CrossesDevices` (or `SettleOptions::copy`)
  configs and jobs are copied: marker `.<what>.moving` (written and
  synced) in the local folder, `copy_tree` into `.<what>.incoming` (each
  file `create_new`, `sync_all`, read back and compared, permissions
  after, links refused), rename into place, **commit** = the old folder
  renamed to `.<what>.moved` beside it, then the marker removed, then the
  old folder deleted. A start that finds the marker without
  `.<what>.moved` throws the copy away (it was never the data); with it,
  removes the marker; a `.<what>.moved` without a marker is deleted. Any
  error before the commit: the copy thrown away, `NotMoved` ("Nothing
  was lost: the app uses them where they are and tries again at its next
  start"), the old place used. Both folders holding one: `BothHold`,
  nothing touched, the local one used, said at every start (an older
  version run after the move writes to the old place: by design not
  merged). **The Python environment is moved by rename only**: a renamed
  venv was checked to work (Python 3.15, Windows: `sys.prefix` the new
  folder, packages import, `-m pip` works; `Scripts\pip.exe` does not,
  the app never runs it), and a copied one too, but a copy of a bare
  venv took 26 s here (thousands of synced, checked files) at a start,
  so across drives it stays and works where it is: `python_left`,
  `SetUpAgain` at each start, `python_environment` = where a setup makes
  the next one; after a successful setup the shell calls
  `remove_left_environment` (only once the choice no longer names it); a
  left environment beside a new one the choice names is removed at the
  next start. A choice naming the old environment's interpreter is
  pointed at the renamed one (`python.yaml` stays in the settings
  folder). Two copies: `.data-moving.lock` exclusive while deciding,
  `.data-running.lock` taken exclusively to move (only when no other copy
  runs) and then held shared for the app's life; a copy started while
  another runs moves nothing and says `OtherCopyRunning` (the data stays
  where it is for both). `File::lock`, let go when a process dies. Tests:
  `tests/data_folders.rs` on temporary folders only (helpers are the test
  binary in the role `settle`, `support::processes`):
  `nothing_to_move_changes_nothing`,
  `everything_moves_whole_and_what_stays_stays`,
  `across_drives_the_data_is_copied_and_the_environment_set_up_again`,
  `a_move_cut_short_anywhere_is_finished_at_the_next_start` (a helper
  killed at six steps: mid-copy, all copied, in place, committed, moved;
  the data whole somewhere each time, then finished),
  `a_local_folder_already_holding_data_is_used_and_the_old_left_alone`,
  `an_unreadable_file_stops_its_move_and_nothing_is_lost` (a file held
  without sharing on Windows, mode 000 on Unix; copy, and on Windows the
  rename too), `two_copies_starting_at_once_move_it_once` (two helpers,
  200 files, a go file), `a_copy_started_while_another_runs_leaves_the_data_where_it_is`,
  `a_moved_python_environment_still_works` (a real venv with the wheel,
  renamed: ready; copied: left, still ready; SKIPPED without Python or
  Node). The headless binary and the Python bridge read nothing that
  moved (credentials, the agent and known hosts stay), so neither moves
  anything nor needs the new place; an unmoved folder works for them as
  before. Uninstalling with "Delete the application data" removes both
  folders whole (Tauri's template: `RmDir /r "$APPDATA\${BUNDLEID}"` and
  `"$LOCALAPPDATA\${BUNDLEID}"`, read in the CLI's 2.12 binary), so the
  moved data and any leftover go too; `check-installer.ps1` already
  pictures both folders and never ticks the box. Not verified: the real
  move on a user's folder (never run by an agent), a roaming folder on
  another drive or a server (played as a copy on one drive), the
  installer check (`npm run test:installer`), a power cut (played as a
  kill: the files are synced before the commit, folders cannot be on
  Windows), macOS. Known limits: across drives the window appears only
  after the copy; a 0.4 copy running beside a starting 0.5 holds no lock
  and may write to the old place after the move (the installer closes
  running copies; portable copies side by side do not).

## The release gate (#76)

- `scripts/release-gate.sh <commit> [tag|dry-run]` is run by the `gate` job of
  `release.yml` before any build: CI (`ci.yml`) green on the commit, and 48
  hours since the newest commit since the previous `v*` tag that changes more
  than docs, the changelog and the version lines of the manifests (a
  dependency bump or a workflow change counts; issue templates do not, so
  the release's own version bump does not restart the wait).
  `npm run test:release-gate` (`scripts/test-release-gate.sh`) builds scratch
  histories for it, a very large lock diff included (a `grep -q` in a
  pipeline once read that as version-only); CI does not run it. `GATE_WAIT_HOURS`
  overrides the 48 (to test it). A build by hand (`workflow_dispatch`) runs it
  as `dry-run`: warnings only.
- The publish job creates the release with `--prerelease` and no longer
  starts `distribute.yml`; the maintainer promotes the release and runs
  Distribute. The rest of the gate (evidence, no tag mid-wave, the `release`
  environment reviewer, the `v*` tag ruleset) is process and settings, in
  `docs/dev/releasing.md`. Not verified: the gate job on a real runner (it
  was run locally against the repository's history).
