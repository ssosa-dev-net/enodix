# enodix documentation

## Using enodix

Start with [Getting started](guide/getting-started.md): install, open a
workspace, add a host and connect, in about ten minutes.

| Guide                                         | What it covers                                                                  |
| --------------------------------------------- | ------------------------------------------------------------------------------- |
| [Install, update and uninstall](guide/install.md) | Installers, checksums, SmartScreen, updates, silent installs, removing everything |
| [The inventory](guide/inventory.md)           | Workspace folders, host files, inherited settings, tags, filters, platforms    |
| [Sessions](guide/sessions.md)                 | SSH, telnet and serial, tabs and split panes, jump hosts, logs, highlighting   |
| [Snippets](guide/snippets.md)                 | Saved commands, variables and how they are filled                              |
| [Multi-send](guide/multi-send.md)             | Typing into a group of terminals at once                                       |
| [Jobs](guide/jobs.md)                         | Running snippets, scripts and file transfers on many devices                   |
| [Config backups](guide/config-backups.md)     | Backing up configurations, history and comparing versions                      |
| [Files](guide/files.md)                       | SFTP and SCP, the Files tab, sending a file to many devices                    |
| [Credentials](guide/credentials.md)           | Credential profiles, named secrets, the vault and Windows Credential Manager   |
| [Legacy crypto](guide/legacy-crypto.md)       | Reaching old devices that only speak outdated SSH algorithms                    |
| [The window and shortcuts](guide/interface.md) | The window, panes and tabs, the palette, keyboard shortcuts           |
| [The headless `enodix-cli`](guide/cli.md)     | Running jobs and backups from a shell or a scheduler                           |
| [Linux](guide/linux.md)                       | The Linux packages and what is known about them                                |
| [Python and Nornir](../python/README.md)      | Your workspace as a Nornir inventory, scripts, Fill in from devices            |

[Where enodix keeps your passwords, and what it connects to](trust.md) lists
every place a login is stored and every connection the app makes.

[demo-workspace](demo-workspace) is a small example network to open in enodix
and look around. The screenshots in these pages come from it.

## Working on enodix

[CONTRIBUTING.md](../CONTRIBUTING.md) says how to build and test enodix and
how pull requests are reviewed. These pages are for maintainers:

- [The containerlab device suite](dev/containerlab.md): the real-device tests
  behind the tested platforms list.
- [Linux checks](dev/linux.md): how the Linux packages are built and checked.
- [Making a release](dev/releasing.md): what a release contains, the
  updater key and publishing with GitHub Actions.
