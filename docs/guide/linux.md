# enodix on Linux

The same app runs on Linux, packaged as a .deb, an .rpm and an AppImage.
This page says how it differs from the Windows one and where things are.
For installing, see [Install, update and uninstall](install.md).

## Install

- **Debian and Ubuntu**: `sudo apt install ./enodix_<version>_amd64.deb`
- **Fedora, RHEL and openSUSE**: install the .rpm with your package
  manager, for example `sudo dnf install ./enodix-<version>-1.x86_64.rpm`.
- **AppImage**: `chmod +x` the file, then run it. Nothing is installed.

The app appears in the application menu with its icon.

## How it differs from Windows

- **Logins.** There is no Linux keychain backend: the app's own encrypted
  vault is the only store. Until it is turned on the app lists no profiles,
  and saving a profile explains that and offers to turn the vault on (the
  profile you typed is then saved into it). Turning the vault off is
  refused with an explanation, since there is no other store to move the
  profiles to. `enodix-cli` with the app closed and no vault says the same.
- **Updates.** The update check only announces a new version, with a link
  to its page; it never installs. Packages are updated with the package
  manager, and the notice of a copy installed from the .deb or the .rpm
  says so. An AppImage is updated by downloading the new one, and its
  notice says it is a portable copy.
- **SSH agent.** Agent profiles use the agent that `SSH_AUTH_SOCK` names;
  there is no default otherwise. Start the app from a session where it is
  set.
- **Serial ports** are the system's (`/dev/ttyUSB0`, `/dev/ttyS0`, ...).
  Opening one usually needs your user in the `dialout` group.
- **Light and dark.** With the theme on "system", the .deb and the .rpm
  follow the desktop's light and dark switch while they run. The AppImage
  picks Adwaita light or dark when it starts, from the desktop's colour
  scheme (GNOME, and desktops that announce one through the desktop portal)
  or its style (XFCE's Appearance, `Adwaita-dark`, or any style whose name
  ends in `-dark`). GNOME's colour scheme switched while it runs is
  followed; a style switched while it runs (XFCE) is not: restart the
  AppImage. Set `APPIMAGE_GTK_THEME=Adwaita:dark` (or `:light`) before
  starting it to override both.

## Where things are

The packages install:

- `/usr/bin/enodix` and `/usr/bin/enodix-cli` (on the `PATH`, where the
  Python bridge finds it);
- the Python wheel in `/usr/lib/enodix/python/`;
- the licence (the GPL) as `/usr/share/doc/enodix/copyright` (.deb) or
  `/usr/share/licenses/enodix/LICENSE` (.rpm, and inside the AppImage),
  with the third-party notices beside it as `THIRD-PARTY-NOTICES.txt`.

Settings, the vault, known hosts, logs, job results, config backups and the
Python environment all live in one folder, `~/.local/share/dev.enodix.app`
(`$XDG_DATA_HOME`).

The AppImage carries the binary and the wheel inside it, so scripts run
outside the app need `ENODIX_CLI` set (see [Python and
Nornir](../../python/README.md)).

## Remove it

Remove the package with the package manager:

- `sudo apt remove enodix` (or `sudo dpkg --purge enodix`) for the .deb;
- `sudo dnf remove enodix` (or `sudo rpm -e enodix`) for the .rpm.

That takes away the program, `enodix-cli`, the wheel, the licence, the
notices and the menu entry. For the AppImage, delete the file.

Your data stays in `~/.local/share/dev.enodix.app` either way, so a later
install finds it. Delete that folder to remove settings, the vault, known
hosts, logs, job results, config backups and the Python environment too.
Nothing is kept in a system keychain on Linux.

## Limits

- Linux is not claimed as supported yet: the packages have been checked in
  XFCE and GNOME desktop sessions inside WSL, and the .rpm in a Fedora
  container, but not on a Linux computer of its own.
- A USB serial adapter, KDE and high-DPI scaling have not been tried.
