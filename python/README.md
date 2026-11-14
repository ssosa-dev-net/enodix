# netterm for Python

Your netterm workspace as a [Nornir](https://nornir.tech) inventory, so that
existing Nornir, Netmiko, scrapli and NAPALM scripts run against it from an
ordinary shell, with the logins of your credential profiles: no password in
a script, a file or an environment variable.

The package is pure Python (3.10 or newer). It reads everything through the
app's headless binary, `netterm-cli`, and needs nothing else; the Nornir
plugin needs Nornir, which you install yourself (with the libraries you
use).

## Installing

The wheel ships with the app, in the `python` folder beside `netterm.exe`
(and in the portable zip), so its version always matches the app's. Install
it from that file, not from a package index:

```powershell
pip install "$env:LOCALAPPDATA\netterm\python\netterm-<version>-py3-none-any.whl"
pip install nornir nornir-netmiko   # or nornir-scrapli, nornir-napalm
```

On Linux the .deb and .rpm packages put the wheel in
`/usr/lib/netterm/python/` and `netterm-cli` in `/usr/bin`:

```sh
pip install /usr/lib/netterm/python/netterm-<version>-py3-none-any.whl
```

The package finds `netterm-cli` through the `NETTERM_CLI` variable if set
(the app sets it for scripts it starts), else at the Windows installer's
location (`%LOCALAPPDATA%\netterm\netterm-cli.exe`), else on `PATH` (where
the Linux packages put it). If it is in none of them, it says where it
looked: set `NETTERM_CLI` to the binary's path (for example inside the
portable folder). The AppImage keeps both inside the image: for scripts run
outside the app, install a package instead, or unpack the image
(`--appimage-extract`) and use `squashfs-root/usr/bin/netterm-cli` and the
wheel in `squashfs-root/usr/lib/netterm/python/`.

## The Nornir inventory

```python
from nornir import InitNornir
from nornir_netmiko.tasks import netmiko_send_command

nr = InitNornir(
    inventory={
        "plugin": "NettermInventory",
        "options": {"workspace": r"C:\Work\inventory", "folder": "Europe/Madrid"},
    }
)
print(nr.run(task=netmiko_send_command, command_string="show version"))
```

Options:

| Option        | Meaning                                                                |
| ------------- | ---------------------------------------------------------------------- |
| `workspace`   | The workspace folder (required).                                       |
| `folder`      | Only the hosts in this folder and the folders below it.                |
| `filter`      | Only the hosts of this saved filter.                                   |
| `hosts`       | Only these host ids (a list), such as `Madrid/core-r1.yaml`.           |
| `credentials` | `True` (default): hand over each host's login; `False` reads no store. |
| `tunnels`     | Tunnel to the hosts behind jump hosts (default: as `credentials`).     |

Give at most one of `folder`, `filter` and `hosts`.

What you get:

- **Hosts** are named by their host id (the host file's path in the
  workspace, unique), with `hostname` = the management address, and `port`
  and `platform` as the app resolves them, inheritance included. The platform
  is passed on as written: use the name your library expects (`arista_eos`,
  `cisco_ios`, ...); the app's platform definitions accept those names too.
- **Groups** are the folders, named by their path (`Europe/Madrid`); a
  folder's parent group is the folder above it, and a host is in its own
  folder's group. Group data: `description` and the folder's own `tags`.
- **Host data**: the custom fields, the built-in fields that are set
  (`vendor`, `model`, `serial_number`, `site`, `building`, `room`, `rack`,
  `rack_unit`, `role`, `description`), `tags` (every tag that applies,
  inherited ones included), and `netterm` with the rest: `id`, `name`,
  `folder`, `folders`, `settings` (every resolved setting), `login`
  (`password`, `private_key`, `agent` or `None`), `jump_hosts`,
  `jump_chain_error`, `behind_jump_host`, `tunnel`, `tunnel_error` (see
  below), `withheld` and `custom` (all the custom fields; a custom field
  with a built-in field's name is only here).
  Custom fields whose names say they hold a secret are never exported (they
  are listed in `withheld`). Filter as usual:
  `nr.filter(F(tags__contains="core"))`, `nr.filter(site="MAD")`.
- **Logins** come from each host's credential profile, read once per
  profile:
  - a password profile sets `username` and `password`;
  - a key profile sets `username` and, in the `netmiko`, `scrapli` and
    `napalm` connection options, the key file and its passphrase
    (`use_keys`/`key_file`/`passphrase`, `auth_private_key`/
    `auth_private_key_passphrase`, `optional_args`);
  - an agent profile sets only `username`: the library uses your SSH agent
    itself (Netmiko and NAPALM get `allow_agent`);
  - an enable secret goes to those options too (`secret`,
    `auth_secondary`).

  A host without a profile has no login. A profile that is not set up on
  this computer stops the load and names the host: set it up, limit the
  inventory, or load with `credentials: False`.

Credentials come from the running app (unlocked) like from an SSH agent;
with the app closed, from the keychain, or from the vault after asking for
its master password in your terminal. Without a terminal and without the
running app, a vault is refused: unattended runs against a vault are
deliberately impossible.

### Hosts behind jump hosts

The libraries make their own SSH connections and know nothing of the app's
jump hosts, so for the hosts behind them the plugin starts
`netterm-cli tunnel`: it connects each jump chain as the app does (the
jump hosts' stored credential profiles or your agent, the host keys the
app trusts, legacy crypto, connect timeouts) and listens on a port of
`127.0.0.1` per host, only there. Those hosts get `hostname` `127.0.0.1`
and that `port`, so scripts run unchanged; hosts behind the same jump host
share its connection. The data keeps `netterm.behind_jump_host` and
`netterm.jump_hosts`, and `netterm.tunnel` says where the tunnel listens.
The tunnels close when the script ends, however it ends, killed included
(the binary serves only while the pipe the plugin holds stays open).
`netterm.tunnels(...)` opens them without Nornir.

Nothing is asked and nothing is trusted on your behalf. A host is not
tunnelled, keeps its own address and carries the reason in
`netterm.tunnel_error` (and a warning names it) when:

- its chain is broken, or a jump host on it has no stored credential
  profile, or asks a question at login (a one-time code, say);
- a jump host's key is not the one the app trusts, or the app has never
  trusted one (the reason names its fingerprint);
- the app has not trusted the **device's** host key yet: connect to it once
  in the app (Trust and remember);
- it is reached over telnet: only SSH is tunnelled, so telnet hosts behind
  jump hosts stay out of reach of scripts.

Leave such hosts out with `nr.filter(F(netterm__tunnel_error=None))`.

The device's host key is checked by the library, which now sees
`127.0.0.1` and the tunnel's port: the plugin writes a known-hosts file per
tunnelled host with the keys the app trusts for the device (as
`127.0.0.1 <key>` and `[127.0.0.1]:<port> <key>`) and turns checking on in
the connection options: Netmiko (`ssh_strict`, `alt_key_file`), scrapli
(`auth_strict_key`, `ssh_known_hosts_file`; its paramiko transport ignores
the port, hence a file per host) and NAPALM (`optional_args`, which the
drivers that log in through Netmiko pass on: `ios`, `nxos_ssh`, and `eos`
when its `transport` is `ssh`). A device whose key changed is refused.
NAPALM's other drivers (`junos`, `iosxr`, ...) do not check the key
through the tunnel.

A tunnel reaches the host's SSH port and nothing else. A library that
talks to another port is pointed at the SSH port all the same and fails:
NAPALM's `eos` driver uses eAPI over HTTPS unless given
`optional_args: {transport: ssh}`, and NETCONF must run on the SSH port
(not 830), as Junos allows. While a script runs, any local program
can reach the devices' SSH ports through the tunnels (as with `ssh -L`); it
still needs their logins.

If a jump host goes down mid-run, the connections through it break at
once (the library reports a closed socket) and the reason is logged as a
`netterm` warning; the next connection tries the chain again.

Other limits: the libraries keep their own host-key handling for hosts
reached directly (they do not read the app's known hosts); telnet and
serial hosts are exported as they are (see `netterm.settings.protocol`),
with no telnet-specific driver names.

## Script jobs

The app runs a script of the workspace (any `.py` file in the workspace
folder or below it) as a job: "Run script…" on a folder, a saved filter or a
host. It uses the Python chosen in Preferences → Python, runs the script
once in the workspace folder, shows its output (stdout and stderr) as it
comes and keeps it with the other job results; exit status 0 counts as
succeeded, anything else as failed. Cancel ends the process.

The script finds the job in its environment:

| Variable              | Meaning                                                   |
| --------------------- | --------------------------------------------------------- |
| `NETTERM_WORKSPACE`   | The workspace folder: the default `workspace` everywhere. |
| `NETTERM_JOB_TARGETS` | A JSON file naming the job's hosts (`hosts`: host ids).   |
| `NETTERM_CLI`         | The headless binary beside the app.                       |

While `NETTERM_JOB_TARGETS` is set, the binary (and so this package) shows
only the job's hosts and hands over only their logins: a folder, filter or
host limit of the script's own narrows that further, a host outside the job
is refused, and so is any other workspace (the workspace's named secrets
stay readable). This guards against mistakes, not against the script: it
runs as you and could undo it. So in a job this is enough:

```python
from nornir import InitNornir

nr = InitNornir(inventory={"plugin": "NettermInventory"})
```

The script has no console: a vault the running app has not unlocked is
refused rather than asked for. Pre-flight leaves out the hosts behind jump
hosts that cannot be tunnelled (a broken chain, a jump host with no stored
profile, telnet), with the reason.

## Fill in from devices

"Fill in from devices…" on a folder (or "Fill in from device…" on a host)
asks each device for its vendor, model, serial number and OS version. The
app runs this package's own script, `python -m netterm.enrich`, as a script
job with the Python chosen in Preferences; it needs
[NAPALM](https://napalm.readthedocs.io/) in that Python (`pip install
napalm`, or tick NAPALM in the app's Python setup). Pre-flight keeps the
hosts reached over SSH with a stored credential profile whose platform
definition answers to a NAPALM driver (hosts behind jump hosts too, through
the tunnels above, unless a jump host has no stored profile or the chain is
broken) (`ios`, `eos`,
`junos`, `nxos_ssh`, `nxos`, `iosxr`, `iosxr_netconf`; NX-OS prefers
`nxos_ssh`). Each device is asked with its own login, on the host's SSH
port (EOS over SSH, not eAPI; Junos over NETCONF on that port), at most 10
at once. The script writes what it found to the file `NETTERM_FACTS_FILE`
names; nothing is written to the workspace until you review it in the app
and apply: empty fields are filled, typed values are replaced only on the
hosts you tick. The OS version goes to the custom field `os_version`.

Verified with NAPALM 5.2 against cEOS 4.36, IOS-XE 17.3 (CSR 1000v) and
cJunos Evolved 25.4.

## Without Nornir

```python
import netterm

token = netterm.secret(r"C:\Work\inventory", "netbox-token")  # a named secret
export = netterm.inventory(r"C:\Work\inventory", filter="Edge")  # the binary's JSON
login = netterm.credentials(r"C:\Work\inventory", "Madrid/core-r1.yaml")
with netterm.tunnels(r"C:\Work\inventory", folder="Madrid") as opened:
    print(opened.hosts)  # host id -> address, port, known_hosts (or error)
```

Errors are `netterm.NettermError` (`CliNotFound`, or `CliFailed` with the
binary's `status` and `reason`).
