# enodix on Linux

The core, the headless binary `enodix-cli` and the Python bridge are
checked on Linux as well as on Windows (#56), and the app is built and
packaged for Linux (#57): a .deb, an .rpm and an .AppImage.

## Checks

```
npm run test:linux
```

runs, on Linux:

1. `cargo clippy -p enodix-core -p enodix-cli --all-targets -- -D warnings`
2. `cargo test -p enodix-core -p enodix-cli`
3. `npm run test:python` (`node scripts/test-python.mjs`)

On Linux it runs in the checkout. On Windows it runs in WSL: the working
tree is copied with rsync (following `.gitignore`, so no `target/`, no
`node_modules` and no keys) into `~/enodix-linux-check` inside the
distribution, and the checks run there. Building from `/mnt/<drive>` would
be slow and would write a Linux `target/` into the Windows checkout.
`ENODIX_WSL_DIR` changes the folder, `ENODIX_WSL_DISTRO` the
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
# to check what the packages hold and add the AppImage's hook (the
# release does):
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
  move the profiles to. `enodix-cli` with the app closed and no vault
  says the same.
- **Updates.** The update check only announces a new version (with a link
  to its page); it never installs. Packages are updated with the package
  manager, and the notice of a copy installed by the .deb or the .rpm says
  so (the packages carry `/usr/lib/enodix/package.txt`); an AppImage is
  updated by downloading the new one, and its notice says it is a portable
  copy.
- **AppImage.** Its runtime sets `PYTHONHOME`, `PYTHONPATH`,
  `LD_LIBRARY_PATH` and others to its own folder for the app; the
  programs the app starts (Python, scripts, the headless binary) get the
  environment without them, or the system's Python could not start.
- **AppImage, light and dark.** With the theme on "system", the .deb and
  the .rpm follow the desktop's light and dark switch while they run. The
  AppImage's GTK is pinned to Adwaita light or dark when it starts (its
  runtime's hook, as other GTK themes break inside the bundle): it reads
  the desktop's colour scheme (GNOME, and desktops that announce one
  through the desktop portal) and, since #143, the desktop's style (XFCE's
  Appearance, `Adwaita-dark`, or any style whose name ends in `-dark`).
  It starts dark on a dark desktop either way. GNOME's colour scheme
  switched while it runs is followed; a style switched while it runs
  (XFCE) is not: restart the AppImage. `APPIMAGE_GTK_THEME=Adwaita:dark`
  (or `:light`) set before starting it overrides both.
- **SSH agent.** Agent profiles use the agent `SSH_AUTH_SOCK` names (there
  is no default otherwise).
- **Serial ports** are the system's (`/dev/ttyUSB0`, `/dev/ttyS0`, ...);
  opening one usually needs the user in the `dialout` group.
- **Where things are.** The packages install `/usr/bin/enodix`,
  `/usr/bin/enodix-cli` (on the PATH, where the Python bridge finds it)
  and the Python wheel in `/usr/lib/enodix/python/`, and the licence (the
  GPL) as `/usr/share/doc/enodix/copyright` (.deb) or
  `/usr/share/licenses/enodix/LICENSE` (.rpm, and inside the AppImage);
  the release checks each package holds those. Settings, the vault,
  known hosts, logs, job results, config backups and the Python
  environment live in `~/.local/share/dev.enodix.app` (`$XDG_DATA_HOME`),
  one folder (Windows splits them between a roaming and a local folder
  since 0.5.0; nothing moves on Linux). The AppImage carries
  the binary and the wheel inside it: scripts run outside the app need
  `ENODIX_CLI` (see `python/README.md`).

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
  is the Windows installer's; elsewhere it is found through `ENODIX_CLI`
  or `PATH`.
- The smoke test drives WebView2 and is Windows-only. It plays the
  keychain-less system on Windows (`ENODIX_CREDENTIAL_STORE=none`). On
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
(the tree is copied as for `test:linux`, `ENODIX_WSL_DIR` and
`ENODIX_WSL_DISTRO` apply). It drives the app over WebDriver:
`tauri-driver` in front of WebKitGTK's `WebKitWebDriver`. WebDriver clicks,
double-clicks and types through the web view's input path and refuses a
click on a control something is drawn over, and the script reads the page
to check what is shown. It starts the example SSH server (with telnet),
an `ssh-agent` of its own holding the test key, a release server for the
update check and the app with a temporary data folder and temporary XDG
folders (`~/.local/share/dev.enodix.app` is never touched), on ports
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
(`sudo apt install ./enodix_<v>_amd64.deb`, built with
`npm run release:linux -- --unsigned`).

Verified on 2026-11-21: Windows 11, WSL 3.0.5, WSLg 1.0.79, Ubuntu 26.04.1,
WebKitGTK 2.52.6, GTK 3.24.52, tauri-driver 2.1.0, Python 3.14.4; enodix
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
| 1. Install, start              | Checked: the .deb installs and the app starts from `/usr/bin/enodix`; the menu entry (`/usr/share/applications/enodix.desktop`, Exec, Icon, `Categories=Utility;`, which was empty before #114) and its 32 and 128 px icons are installed. The AppImage starts (`chmod +x`, then run; FUSE works in WSL). Not seen: the entry in a desktop's application menu (WSLg has none).                                                                                                                |
| 2. Workspace, views, themes    | Checked: a workspace opened by its folder, the tree, the details sheet, the palette (Ctrl+K, a host found), Preferences. Light and dark follow the desktop's GTK theme (`GTK_THEME=Adwaita:dark` gives the dark app; the AppImage's runtime sets `GTK_THEME` itself from the desktop portal's colour scheme, `APPIMAGE_GTK_THEME` overrides it). Not seen: a desktop's own light/dark switch.                                                                                                   |
| 3. Profiles, the vault         | Checked: Security says the vault is the only store; saving a profile explains and turns the vault on; key and agent profiles saved; turning it off says there is no other store; after a restart the unlock screen asks for the master password and unlocks.                                                                                                                                                                                                                                  |
| 4. SSH, telnet, serial         | Checked: SSH with a password, a key and an agent profile (`ssh-agent`, `SSH_AUTH_SOCK`), the host key question; telnet. Serial cannot be checked: WSL has no serial adapter.                                                                                                                                                                                                                                                                                                                  |
| 5. Terminal                    | Checked: typing, Ctrl+Shift+C / Ctrl+Shift+V through the system clipboard (under WSLg shared with Windows), WebGL rendering, zoom (Ctrl+= / Ctrl+0), split right, several tabs, multi-send into two sessions. Not driven: paste with a right click (off by default).                                                                                                                                                                                                                          |
| 6. Python                      | Checked: the setup finds the system's `/usr/bin/python3` and builds the environment from the package's wheel (`/usr/lib/enodix/python/`, the AppImage's own inside it); a script job runs (it gets its login from the app); `enodix-cli inventory <workspace>` from a shell. Fixed in #114: from the AppImage no Python was found (its runtime's `PYTHONHOME` leaked into the child).                                                                                                        |
| 7. Updates                     | Checked: with a newer `latest.json` the notice offers only the release page, no install; the .deb's copy says to update with the package manager, the AppImage's that it is a portable copy (both said "unpack it over this one" before #114).                                                                                                                                                                                                                                                 |
| 8. Log folder, web links       | Checked as far as the app goes: "Open log folder" and a web link hand the folder and the URL to the desktop's opener (under WSL, Windows' through `powershell.exe`, with the folder as `\\wsl$\<distribution>\...`). Not seen: a file manager or browser opening (replaced by the recording stand-ins).                                                                                                                                                                                         |
| 9. The app's own frame         | Checked on Wayland and X11: no system frame (`is_decorated` false, and no title bar in the X11 pictures), Minimize, Maximize, Close in the top bar; Maximize and Restore; a double click on the top bar's empty part maximizes and restores; Close ends the app. Cannot be checked under WSLg: dragging the window, resizing from the edges and corners, Minimize's effect and edge tiling. WSLg's compositor moves and sizes windows for the Windows desktop, Wayland has no position to read, XWayland keeps reporting the old one, and `is_minimized` stays false. |

WSLg alone could not show the application menu entry, serial, a file
manager and a browser opened by the app, the desktop's light/dark
switch, a GPU, the frame's moving, resizing, minimizing and edge tiling,
and the .rpm on Fedora. The next section takes those as far as WSL goes
(#58).

## A desktop out of sight (#58)

Inside WSL the check can run in a real desktop session of its own, on a
virtual screen: nothing appears on the Windows desktop, and the
computer's keyboard and pointer are left alone.

```
npm run test:linux-desktop -- --desktop xfce          # the .deb, X11
npm run test:linux-desktop -- --desktop gnome         # the .deb, Wayland
npm run test:linux-desktop -- --desktop xfce --app <file>.AppImage
npm run test:linux-desktop -- --desktop gnome --app <file>.AppImage
npm run test:linux-desktop -- --desktop xfce --gallium-driver d3d12  # the GPU
npm run test:linux-fedora                             # the .rpm on Fedora 45
```

`--gallium-driver d3d12` gives the app Mesa's D3D12 driver (WSL's way to
the computer's GPU) and requires the renderer webkit://gpu names to be
it; without it the check reports the renderer it finds.

- `--desktop xfce`: an X11 session on Xvfb with XFCE's window manager
  (xfwm4), its panel (the Whisker menu, the task list), its settings
  daemon and the desktop portal. The pointer and keys are X's own
  (xdotool); where a window is and whether it is minimized or maximized
  is read from the window manager.
- `--desktop gnome`: GNOME Shell on Wayland, headless with a virtual
  monitor. The pointer and keys go through Mutter's remote desktop
  interface (`scripts/linux-desktop-input.py`), as GNOME's own remote
  desktop does; the windows are read from the shell.
- `npm run test:linux-fedora` builds a Fedora container (podman) with the
  .rpm installed by dnf, so its dependencies resolve as on Fedora, and
  runs the whole check with the app inside it on the XFCE session
  (`--desktop gnome` for GNOME). It first checks the package's menu
  entry and icons in the image.

Each session has its own D-Bus bus, runtime folder and settings folders
in the check's temporary folder; the default applications are set there
to Thunar (folders) and GNOME Web (web pages). The whole check list of
the previous section runs in it (also without `--desktop`, a serial
console now: see the table below), and with a desktop also:

1. the menu entry: `desktop-file-validate`, the icon as GTK's icon theme
   finds it at 16 to 256 px, the menu's search (Whisker; GNOME's
   overview) finding enodix and starting it (GNOME: the window tied to
   `enodix.desktop`, so the dock shows its icon);
2. the desktop's own dark switch while the app runs, its theme on
   "system" (XFCE: Appearance's style, `Adwaita-dark`; GNOME: the colour
   scheme), and back to light; then, at the restart, the app started with
   the desktop already dark is dark. For the AppImage on XFCE only what it
   starts with is required (light, then dark at the restart): a style
   switched while it runs is noted, not expected to reach it (#143);

3. the frame with the desktop's pointer: the top bar dragged, the window
   resized from each edge and corner, tiled to the left half by dragging
   it to the screen's edge and with the desktop's key (Super+Left), back
   to its size when dragged off, maximized and restored by a double
   click, minimized with its button and brought back;
4. "Open log folder" and a web link really starting Thunar and GNOME
   Web: the file manager shows the log folder, the browser loads the
   page (the check's server sees it), and neither got anything of the
   app's AppImage in its environment or its working folder.

Setting up, once (as root in the distribution), besides the packages of
the previous section:

```
apt-get install --no-install-recommends xvfb xfwm4 xfce4-panel \
  xfce4-settings xfce4-whiskermenu-plugin thunar epiphany-browser \
  gnome-shell socat desktop-file-utils podman uidmap wmctrl
```

`npm run test:linux-fedora` pulls `registry.fedoraproject.org/fedora:45`
(about 200 MB) and builds `enodix-fedora-check` on it (about 1.2 GB);
`podman image rm enodix-fedora-check fedora:45` takes them away again.

Verified on 2026-11-22: Windows 11, WSL 2, Ubuntu 26.04.1, WebKitGTK
2.52.6, xfwm4 4.20.0, GNOME Shell 50.1; Fedora 45 with WebKitGTK 2.54.2
in podman 5.7. enodix 0.4.1 as built from this tree. Run again on the
same day after #140, #141, #142 and #143: all five runs above (without
`--gallium-driver`) passed.

| #58 item                     | In WSL                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Menu entry and icon          | Checked on XFCE and GNOME: valid, found by each menu's search with its icon, started from it; GNOME ties the window to the entry. The package installs the icon at 32 and 128 px only; the other sizes are scaled from those. Not seen: KDE's menu.                                                                                                                                                                                                                                                                                                                                                                       |
| Light and dark               | Checked: the .deb follows XFCE's style switch and GNOME's colour scheme live, both ways, and starts dark on a dark desktop. The AppImage follows GNOME's colour scheme live. Its runtime pins `GTK_THEME` to `Adwaita:light` or `:dark` at start; since #143 enodix's own start-up hook reads XFCE's style as well, so it starts light on a light XFCE and dark on a dark one, but a style switched while it runs needs a restart (the check notes that it stays light). Before #143 a dark XFCE style left it light. |
| Serial                       | Partly: a console on a pseudo terminal that `socat` forwards (as a console server's port is reached) opens from quick connect by its path at 115200, shows the console's output, takes typing, and takes Send break with the session going on. The core's tests drive the real driver on a pseudo terminal pair (`tests/serial_pty.rs`). Fixed: such a port could not be opened at all (RTS refused). Not checkable in WSL: a USB adapter and its driver, the speed on a wire, a break, DTR and RTS reaching a device, unplugging, the `dialout` group. |
| File manager and browser     | Checked on XFCE and GNOME, from the .deb and the AppImage: Thunar opens on the log folder, GNOME Web loads the web link and the release page. Fixed: from the AppImage they got its library path, GTK and GIO modules, Python home and more, and started inside its mount.                                                                                                                                                                                                                                                                                                                                          |
| WebGL and a GPU              | Partly. By default Mesa renders in software (llvmpipe): the user cannot open `/dev/dri` (the `render` group) and the virtual displays have no DRI3. With `--gallium-driver d3d12` the app's GL is Mesa's D3D12 driver on the computer's own GPU through `/dev/dxg`: webkit://gpu says `GL_RENDERER` "D3D12 (<the GPU>)", WebGL on, and the terminal draws with xterm's WebGL renderer, on X11 and on Wayland. That is a real GPU through a translation layer (OpenGL ES over Direct3D 12), and WebKit hands its frames over in shared memory (no render node for DMA-BUF). Every check passes with it, but the pictures show stray diagonal lines across some rounded borders (buttons, badges), which llvmpipe does not draw: the translation layer's, not the app's, and why WSL's default stays software. Not checkable in WSL: Mesa's or a vendor's native driver (amdgpu, i915/xe, nouveau, NVIDIA's) and zero-copy DMA-BUF. |
| The frame                    | Checked on X11 (xfwm4) and Wayland (GNOME's Mutter): moved, resized from all four edges and corners, tiled by drag and key, restored, maximized and restored by a double click, minimized and brought back, all with the desktop's pointer and read from the window manager. On Wayland the app cannot tell it is minimized (`is_minimized` stays false: Wayland does not say). xfwm4 tiles on drag only with "wrap windows to the next workspace" off (Thunar the same). Not seen: KWin and other window managers, HiDPI scaling. |
| The .rpm on Fedora           | Checked in a Fedora 45 container: dnf installs the .rpm with its dependencies (WebKitGTK 2.54.2), its menu entry is valid with its icons, and the whole check passes with the app from it on the XFCE session: the update notice says a package manager updates it, Python from Fedora's own, serial, Thunar and GNOME Web from Fedora. The container shares the kernel, the X display and the session bus with WSL, so this is Fedora's userland, not a Fedora desktop: Fedora's GNOME session, SELinux and its packaging of the desktop are not in it. A second WSL distribution would add a systemd user session but no more than that, and was not installed. |

What WSL cannot show at all: a USB serial adapter, a GPU without the
D3D12 translation, KDE, and a person's hand on a real pointer (the
checks inject input at the X server or compositor, the layer real
devices reach).

## Checking the packages on a Linux desktop

On a real desktop (for example Ubuntu and Fedora, X11 and Wayland), run
`npm run test:linux-desktop` there if it can be set up, and check by hand
what it cannot (see the table above):

1. Install the .deb (`sudo apt install ./enodix_<v>_amd64.deb`) or the
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
   environment from `/usr/lib/enodix/python/`; a script job runs and
   `enodix-cli inventory <workspace>` works from a shell.
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
