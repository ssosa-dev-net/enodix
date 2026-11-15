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
  manager, an AppImage by downloading the new one.
- **SSH agent.** Agent profiles use the agent `SSH_AUTH_SOCK` names (there
  is no default otherwise).
- **Serial ports** are the system's (`/dev/ttyUSB0`, `/dev/ttyS0`, ...);
  opening one usually needs the user in the `dialout` group.
- **Where things are.** The packages install `/usr/bin/netterm`,
  `/usr/bin/netterm-cli` (on the PATH, where the Python bridge finds it)
  and the Python wheel in `/usr/lib/netterm/python/`. Settings, the vault,
  known hosts, logs, job results and config backups live in
  `~/.local/share/dev.netterm.app` (`$XDG_DATA_HOME`). The AppImage carries
  the binary and the wheel inside it: scripts run outside the app need
  `NETTERM_CLI` (see `python/README.md`).

## What is skipped on Linux

- `tests/keychain.rs` and the headless binary's keychain test: the
  keychain store is Windows' Credential Manager only.
- `connection_errors::an_unreachable_network_is_reported`
  (ignored): it relies on Windows' handling of 240.0.0.0/4.
- Pageant, the credential agent pipe's access list, and a vault file that
  cannot be removed (Windows file sharing); the agent's Unix socket mode
  and the binary's terminal prompt have Unix tests of their own.
- The Python package's default install location of the headless binary
  is the Windows installer's; elsewhere it is found through `NETTERM_CLI`
  or `PATH`.
- The smoke test drives WebView2 and is Windows-only. It plays the
  keychain-less system on Windows (`NETTERM_CREDENTIAL_STORE=none`).

Verified on Ubuntu 26.04 under WSL 2 (Rust 1.100, Python 3.14).

## Checking the packages on a Linux desktop

Under WSL only the build and the packages' contents are checked. On a real
desktop (for example Ubuntu and Fedora, X11 and Wayland), check by hand:

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
