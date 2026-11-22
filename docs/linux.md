# netterm on Linux

The core, the headless binary `netterm-cli` and the Python bridge are
checked on Linux as well as on Windows (#56), and the app is built and
packaged for Linux (#57): a .deb, an .rpm and an .AppImage.

## Checks

```
npm run test:linux
```

runs, on Linux:

1. `cargo clippy -p netterm-core -p netterm-cli --all-targets -- -D warnings`
2. `cargo test -p netterm-core -p netterm-cli`
3. `npm run test:python` (`node scripts/test-python.mjs`)

On Linux it runs in the checkout. On Windows it runs in WSL: the working
tree is copied with rsync (following `.gitignore`, so no `target/`, no
`node_modules` and no keys) into `~/netterm-linux-check` inside the
distribution, and the checks run there. Building from `/mnt/<drive>` would
be slow and would write a Linux `target/` into the Windows checkout.
`NETTERM_WSL_DIR` changes the folder, `NETTERM_WSL_DISTRO` the
distribution (default: WSL's default one). `npm run release:linux` uses the
same copy (see `docs/releasing.md`).

## Setting up WSL once

A WSL 2 distribution with network access (`wsl --install Ubuntu`), then
inside it (as root: `wsl -u root`):

```
apt-get install build-essential pkg-config libudev-dev rsync \
  python3 python3-venv nodejs npm
# for the app and its packages (Tauri on Linux):
apt-get install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev libxdo-dev libssl-dev patchelf file wget curl xdg-utils
# to check what the packages hold (the release does):
apt-get install rpm2cpio cpio squashfs-tools
```

and as the user:

```
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy
```

`libudev-dev` is for the `serialport` crate. Rust must be 1.90 or newer.
The Python suite installs Nornir from the package index the first time;
the first package build downloads the AppImage tools.

## How the app differs on Linux

- **Credentials.** There is no Linux keychain backend: the vault is the
  only store (`ActiveStore::without_keychain`). Until it is turned on the
  app lists no profiles, and saving a profile explains that and offers to
  turn the vault on (the profile typed is then saved into it). Turning the
  vault off is refused with an explanation: there is no other store to
  move the profiles to. `netterm-cli` with the app closed and no vault
  says the same.
- **Updates.** The update check only announces a new version (with a link
  to its page); it never installs. Packages are updated with the package
  manager, and the notice of a copy installed by the .deb or the .rpm says
  so (the packages carry `/usr/lib/netterm/package.txt`); an AppImage is
  updated by downloading the new one, and its notice says it is a portable
  copy.
- **AppImage.** Its runtime sets `PYTHONHOME`, `PYTHONPATH`,
  `LD_LIBRARY_PATH` and others to its own folder for the app; the
  programs the app starts (Python, scripts, the headless binary) get the
  environment without them, or the system's Python could not start.
- **SSH agent.** Agent profiles use the agent `SSH_AUTH_SOCK` names (there
  is no default otherwise).
- **Serial ports** are the system's (`/dev/ttyUSB0`, `/dev/ttyS0`, ...);
  opening one usually needs the user in the `dialout` group.
- **Where things are.** The packages install `/usr/bin/netterm`,
  `/usr/bin/netterm-cli` (on the PATH, where the Python bridge finds it)
  and the Python wheel in `/usr/lib/netterm/python/`, and the licence (the
  GPL) as `/usr/share/doc/netterm/copyright` (.deb) or
  `/usr/share/licenses/netterm/LICENSE` (.rpm, and inside the AppImage);
  the release checks each package holds those. Settings, the vault,
  known hosts, logs, job results, config backups and the Python
  environment live in `~/.local/share/dev.netterm.app` (`$XDG_DATA_HOME`),
  one folder (Windows splits them between a roaming and a local folder
  since 0.5.0; nothing moves on Linux). The AppImage carries
  the binary and the wheel inside it: scripts run outside the app need
  `NETTERM_CLI` (see `python/README.md`).

## What is skipped on Linux

- `tests/keychain.rs` and the headless binary's keychain test: the
  keychain store is Windows' Credential Manager only.
- `connection_errors::an_unreachable_network_is_reported`
  (ignored): it relies on Windows' handling of 240.0.0.0/4.
- Pageant, the credential agent pipe's access list, and a vault file that
  cannot be removed (Windows file sharing), and the binary's check for a
  console nobody sees (a window station); the agent's Unix socket mode,
  the client's refusal of a socket others can open or of a path that is
  not a socket, and the binary's terminal prompt have Unix tests of their
  own.
- The Python package's default install location of the headless binary
  is the Windows installer's; elsewhere it is found through `NETTERM_CLI`
  or `PATH`.
- The smoke test drives WebView2 and is Windows-only. It plays the
  keychain-less system on Windows (`NETTERM_CREDENTIAL_STORE=none`). On
  Linux the much smaller `npm run test:linux-desktop` drives the app
  (below).

Verified on Ubuntu 26.04 under WSL 2 (Rust 1.100, Python 3.14).

## The app under WSLg

WSL 2 on Windows 11 runs Linux GUI apps through WSLg: a Wayland
compositor (Weston) with XWayland, shown on the Windows desktop. The app
was installed from the .deb, started from it and from the AppImage, and
driven through the check list below on both backends (#114):

```
npm run test:linux-desktop                         # the installed .deb, Wayland
npm run test:linux-desktop -- --backend x11        # the same through XWayland
npm run test:linux-desktop -- --app <file>.AppImage
```

`scripts/linux-desktop-check.mjs` runs on Linux, or from Windows in WSL
(the tree is copied as for `test:linux`, `NETTERM_WSL_DIR` and
`NETTERM_WSL_DISTRO` apply). It drives the app over WebDriver:
`tauri-driver` in front of WebKitGTK's `WebKitWebDriver`. WebDriver clicks,
double-clicks and types through the web view's input path and refuses a
click on a control something is drawn over, and the script reads the page
to check what is shown. It starts the example SSH server (with telnet),
an `ssh-agent` of its own holding the test key, a release server for the
update check and the app with a temporary data folder and temporary XDG
folders (`~/.local/share/dev.netterm.app` is never touched), on ports
24722-24728. The desktop's openers (`xdg-open` and the others the app
tries; under WSL first `powershell.exe`, which hands links to Windows'
browser) are replaced on the app's PATH by scripts that record what they
were asked to open. Pictures of the window go to `--pictures <folder>`
(default: a folder in the run's temporary one); under X11 also the window
as the X server has it. It needs, once, in the distribution (as root):

```
apt-get install webkitgtk-webdriver xdotool imagemagick openssh-client
# webkit2gtk-driver on releases before Ubuntu 26.04
```

and as the user `cargo install tauri-driver`, then the .deb installed
(`sudo apt install ./netterm_<v>_amd64.deb`, built with
`npm run release:linux -- --unsigned`).

Verified on 2026-11-21: Windows 11, WSL 3.0.5, WSLg 1.0.79, Ubuntu 26.04.1,
WebKitGTK 2.52.6, GTK 3.24.52, tauri-driver 2.1.0, Python 3.14.4; netterm
0.4.1's .deb and AppImage; Wayland, X11 (`GDK_BACKEND=x11`) and the
AppImage on Wayland each passed every check the script makes. Nothing
had to be set for the app to show: no `WEBKIT_DISABLE_DMABUF_RENDERER`,
no `GDK_BACKEND`. The GPU is WSL's virtual one, which the user cannot
reach unless in the `render` group (`/dev/dri/renderD128`): Mesa then
renders with llvmpipe (software), and WebGL is on all the same (the
terminal draws with xterm's WebGL renderer). On Wayland the window comes
up 1100x757 for the 1100x720 asked (X11: 1100x720); nothing is cut off.

| Point                          | Under WSLg                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1. Install, start              | Checked: the .deb installs and the app starts from `/usr/bin/netterm`; the menu entry (`/usr/share/applications/netterm.desktop`, Exec, Icon, `Categories=Utility;`, which was empty before #114) and its 32 and 128 px icons are installed. The AppImage starts (`chmod +x`, then run; FUSE works in WSL). Not seen: the entry in a desktop's application menu (WSLg has none).                                                                                                                |
| 2. Workspace, views, themes    | Checked: a workspace opened by its folder, the tree, the details sheet, the palette (Ctrl+K, a host found), Preferences. Light and dark follow the desktop's GTK theme (`GTK_THEME=Adwaita:dark` gives the dark app; the AppImage's runtime sets `GTK_THEME` itself from the desktop portal's colour scheme, `APPIMAGE_GTK_THEME` overrides it). Not seen: a desktop's own light/dark switch.                                                                                                   |
| 3. Profiles, the vault         | Checked: Security says the vault is the only store; saving a profile explains and turns the vault on; key and agent profiles saved; turning it off says there is no other store; after a restart the unlock screen asks for the master password and unlocks.                                                                                                                                                                                                                                  |
| 4. SSH, telnet, serial         | Checked: SSH with a password, a key and an agent profile (`ssh-agent`, `SSH_AUTH_SOCK`), the host key question; telnet. Serial cannot be checked: WSL has no serial adapter.                                                                                                                                                                                                                                                                                                                  |
| 5. Terminal                    | Checked: typing, Ctrl+Shift+C / Ctrl+Shift+V through the system clipboard (under WSLg shared with Windows), WebGL rendering, zoom (Ctrl+= / Ctrl+0), split right, several tabs, multi-send into two sessions. Not driven: paste with a right click (off by default).                                                                                                                                                                                                                          |
| 6. Python                      | Checked: the setup finds the system's `/usr/bin/python3` and builds the environment from the package's wheel (`/usr/lib/netterm/python/`, the AppImage's own inside it); a script job runs (it gets its login from the app); `netterm-cli inventory <workspace>` from a shell. Fixed in #114: from the AppImage no Python was found (its runtime's `PYTHONHOME` leaked into the child).                                                                                                        |
| 7. Updates                     | Checked: with a newer `latest.json` the notice offers only the release page, no install; the .deb's copy says to update with the package manager, the AppImage's that it is a portable copy (both said "unpack it over this one" before #114).                                                                                                                                                                                                                                                 |
| 8. Log folder, web links       | Checked as far as the app goes: "Open log folder" and a web link hand the folder and the URL to the desktop's opener (under WSL, Windows' through `powershell.exe`, with the folder as `\\wsl$\<distribution>\...`). Not seen: a file manager or browser opening (replaced by the recording stand-ins).                                                                                                                                                                                         |
| 9. The app's own frame         | Checked on Wayland and X11: no system frame (`is_decorated` false, and no title bar in the X11 pictures), Minimize, Maximize, Close in the top bar; Maximize and Restore; a double click on the top bar's empty part maximizes and restores; Close ends the app. Cannot be checked under WSLg: dragging the window, resizing from the edges and corners, Minimize's effect and edge tiling. WSLg's compositor moves and sizes windows for the Windows desktop, Wayland has no position to read, XWayland keeps reporting the old one, and `is_minimized` stays false. |

What only a real Linux desktop can show: the application menu entry,
serial adapters, a file manager and a browser opened by the app, the
desktop's light/dark switch, a GPU the app can use, and the window frame's
moving, resizing, minimizing and edge tiling (#58).

## Checking the packages on a Linux desktop

On a real desktop (for example Ubuntu and Fedora, X11 and Wayland), run
`npm run test:linux-desktop` there if it can be set up, and check by hand
what it cannot (see the table above):

1. Install the .deb (`sudo apt install ./netterm_<v>_amd64.deb`) or the
   .rpm; the app is in the application menu with its icon and starts. Run
   the AppImage too (`chmod +x`, then start it).
2. Open a workspace folder; the tree, side panel, palette (Ctrl+K) and
   preferences work; light and dark themes follow the desktop.
3. Profiles: Preferences, Security says the vault is the only store. Save a
   new profile: the explanation and "Turn on the vault…" appear; turn it on
   there and the profile is saved. Restart: the unlock screen asks for the
   master password. "Turn off the vault…" explains there is no other store.
4. Connect over SSH with a password profile, a key profile and an agent
   profile (`ssh-agent` running, `SSH_AUTH_SOCK` set in the session that
   starts the app); telnet; a serial adapter (`/dev/ttyUSB0`, user in
   `dialout`): listed in quick connect, opens, Send break works.
5. Terminal: typing, copy and paste (Ctrl+Shift+C/V, right click), WebGL
   rendering, split panes, several tabs, zoom, multi-send.
6. Python: Preferences, Python finds the system's Python and sets up the
   environment from `/usr/lib/netterm/python/`; a script job runs and
   `netterm-cli inventory <workspace>` works from a shell.
7. Updates: with a newer `latest.json` at the release address, the notice
   only offers the release page, never an install.
8. Log folders and "Open log folder" open the file manager; web links open
   the browser.
9. The app's own window frame (#80, not checked on Linux yet): no title
   bar from the desktop on the main window, a run window or a config
   history window; dragging the top bar's empty part moves the window and
   a double click maximizes and restores it; Minimize, Maximize/Restore
   and Close work; the window resizes from every edge and corner, and the
   desktop's edge tiling still works (X11 and Wayland).
