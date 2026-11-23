# The headless binary: enodix-cli

`enodix-cli.exe` is a command-line companion to the app, for scripts and
automation. It has no window and does not need the app running. It prints
your inventory as JSON, hands out stored logins and opens tunnels through
jump hosts. It is installed beside `enodix.exe` (and is in the portable
zip); on Linux the packages put `enodix-cli` on the `PATH`.

## Export the inventory

`enodix-cli inventory` prints a workspace's inventory as JSON, each host
with the settings that apply to it (inheritance resolved), its jump hosts,
folders, tags and custom fields. Credential profiles appear by name only,
and custom fields named like a secret (password, key, community, token) are
left out:

```powershell
enodix-cli inventory C:\Work\inventory
enodix-cli inventory C:\Work\inventory --folder Europe/Madrid
enodix-cli inventory C:\Work\inventory --filter "Arista in Madrid"
enodix-cli inventory C:\Work\inventory --host core-r1.yaml --host Lab/sw1.yaml
```

Problems with workspace files are printed as warnings and do not stop the
export. It exits with 1 (and says why) for a missing workspace or an
unknown folder, filter or host, and with 2 for a command line it does not
understand. `enodix-cli --help` lists the options. A value that starts
with `-` goes after `--`, which ends the options
(`enodix-cli secret -- C:\Work\inventory -ro`), or after `=` for an option
(`--folder=-old`, `--host=-spare.yaml`).

## Get a login or a secret

It also hands out logins and opens tunnels, for scripts:

```powershell
enodix-cli credentials C:\Work\inventory Europe/Madrid/core-r1.yaml
enodix-cli secret C:\Work\inventory netbox-token
enodix-cli tunnel C:\Work\inventory --folder Europe/Madrid
enodix-cli tunnel C:\Work\inventory --folder Europe/Madrid --port 830 --port 443
```

`credentials` prints the login of the credential profile a host uses, and
`secret` a named secret of the workspace, as JSON. They come from the
running app when it is open and its vault unlocked; otherwise from the
keychain, or from the vault after asking its master password on the
terminal. A password is never taken from the command line or the
environment, and nothing is written to disk.

## Open tunnels to hosts behind jump hosts

`tunnel` opens a port on `127.0.0.1` for each host behind jump hosts,
through the jump chain the app would use, and serves it until its
standard input ends. It asks no host-key or login question: a jump host or
device whose key the app has not trusted is refused, with the reason in its
output.

Each `--port` adds a port on `127.0.0.1` per host for that device port
(NETCONF on 830, an API on 443, any TCP port), through the same jump
chain; its output lists them as `extra_ports`. Nothing checks that the
device listens there: a connection to a closed port is closed, with the
reason on standard error. The app does not check a device's TLS
certificate through a tunnel; the script decides how to.

## Remove every saved login

`enodix-cli clear-keychain --yes` removes every saved login (credential
profiles and named secrets, of every workspace) enodix keeps in Windows
Credential Manager, and says how many. Without `--yes` it only says how many
it would remove, and removes nothing. The vault is not touched. The
uninstaller runs it when you tick the data box (see [Uninstall](install.md#uninstall)).

## Using it from Python

The Python package in `python/` uses `enodix-cli` for you: see
[Python and Nornir](../../python/README.md).
