# enodix for Python

Your enodix workspace as a [Nornir](https://nornir.tech) inventory, so that
existing Nornir, Netmiko, scrapli and NAPALM scripts run against it from an
ordinary shell, with the logins of your credential profiles: no password in
a script, a file or an environment variable.

The package is pure Python (3.10 or newer). It reads everything through the
app's headless binary, `enodix-cli`, and needs nothing else; the Nornir
plugin needs Nornir, which you install yourself (with the libraries you
use).

## Installing

The wheel ships with the app, in the `python` folder beside `enodix.exe`
(and in the portable zip), so its version always matches the app's. Install
it from that file, not from a package index:

```powershell
pip install "$env:LOCALAPPDATA\enodix\python\enodix-<version>-py3-none-any.whl"
pip install nornir nornir-netmiko   # or nornir-scrapli, nornir-napalm
```

On Linux the .deb and .rpm packages put the wheel in
`/usr/lib/enodix/python/` and `enodix-cli` in `/usr/bin`:

```sh
pip install /usr/lib/enodix/python/enodix-<version>-py3-none-any.whl
```

The package finds `enodix-cli` through the `ENODIX_CLI` variable if set
(the app sets it for scripts it starts), else at the Windows installer's
location (`%LOCALAPPDATA%\enodix\enodix-cli.exe`), else on `PATH` (where
the Linux packages put it). If it is in none of them, it says where it
looked: set `ENODIX_CLI` to the binary's path (for example inside the
portable folder). The AppImage keeps both inside the image: for scripts run
outside the app, install a package instead, or unpack the image
(`--appimage-extract`) and use `squashfs-root/usr/bin/enodix-cli` and the
wheel in `squashfs-root/usr/lib/enodix/python/`.

The package and the binary should be the same version. When they are not
(the app was upgraded and the package in your own environment was not, say),
the package warns once, when it first runs the binary, with a
`enodix.VersionWarning` naming both versions: install the wheel of the
app's version. It goes on all the same; only a change of the binary's JSON
format stops it. Names that start with `-` (a secret called `-ro`, a folder
`-old`) are handed to the binary as names; a binary older than this package
refuses them.

## The Nornir inventory

```python
from nornir import InitNornir
from nornir_netmiko.tasks import netmiko_send_command

nr = InitNornir(
    inventory={
        "plugin": "EnodixInventory",
        "options": {"workspace": r"C:\Work\inventory", "folder": "Europe/Madrid"},
    }
)
print(nr.run(task=netmiko_send_command, command_string="show version"))
```

Options:

| Option         | Meaning                                                                |
| -------------- | ---------------------------------------------------------------------- |
| `workspace`    | The workspace folder (required).                                       |
| `folder`       | Only the hosts in this folder and the folders below it.                |
| `filter`       | Only the hosts of this saved filter.                                   |
| `hosts`        | Only these host ids (a list), such as `Madrid/core-r1.yaml`.           |
| `credentials`  | `True` (default): hand over each host's login; `False` reads no store. |
| `tunnels`      | Tunnel to the hosts behind jump hosts (default: as `credentials`).     |
| `tunnel_ports` | Other device ports the tunnels carry, such as `[830, 443]` (below).    |

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
  inherited ones included), and `enodix` with the rest: `id`, `name`,
  `folder`, `folders`, `settings` (every resolved setting), `login`
  (`password`, `private_key`, `agent` or `None`), `jump_hosts`,
  `jump_chain_error`, `behind_jump_host`, `tunnel`, `tunnel_error`,
  `ports` (see below), `withheld` and `custom` (all the custom fields; a custom field
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
its master password in your terminal. Without a terminal (or with a console
nobody sees, as a scheduled task run while nobody is logged on has) and
without the running app unlocked, a vault is refused at once, saying so:
unattended runs against a vault that is not unlocked in the running app are
deliberately impossible.

### Hosts behind jump hosts

The libraries make their own SSH connections and know nothing of the app's
jump hosts, so for the hosts behind them the plugin starts
`enodix-cli tunnel`: it connects each jump chain as the app does (the
jump hosts' stored credential profiles or your agent, the host keys the
app trusts, legacy crypto, connect timeouts) and listens on a port of
`127.0.0.1` per host, only there. Those hosts get `hostname` `127.0.0.1`
and that `port`, so scripts run unchanged; hosts behind the same jump host
share its connection. The data keeps `enodix.behind_jump_host` and
`enodix.jump_hosts`, and `enodix.tunnel` says where the tunnel listens.
The tunnels close when the script ends, however it ends, killed included
(the binary serves only while the pipe the plugin holds stays open).
`enodix.tunnels(...)` opens them without Nornir.

Nothing is asked and nothing is trusted on your behalf. A host is not
tunnelled, keeps its own address and carries the reason in
`enodix.tunnel_error` (and a warning names it) when:

- its chain is broken, or a jump host on it has no stored credential
  profile, or asks a question at login (a one-time code, say);
- a jump host's key is not the one the app trusts, or the app has never
  trusted one (the reason names its fingerprint);
- the app has not trusted the **device's** host key yet: connect to it once
  in the app (Trust and remember);
- it is reached over telnet: only SSH is tunnelled, so telnet hosts behind
  jump hosts stay out of reach of scripts. They are marked
  `enodix.behind_jump_host` all the same (`jump_hosts` stays empty: the
  app works out a chain only for SSH; `enodix.settings.jump_host` names
  the jump host).

Leave such hosts out with `nr.filter(F(enodix__tunnel_error=None))`.

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

#### Other ports: NETCONF, HTTPS APIs

A tunnel reaches the host's SSH port, and the other device ports you name
with the `tunnel_ports` option (`enodix.tunnels(..., ports=[...])`
without Nornir): any TCP port, such as 830 for NETCONF or 443 for eAPI,
RESTCONF or NX-API. They go through the same jump chain and the same jump
connection as the SSH port, for every tunnelled host of the inventory,
each on a port of `127.0.0.1` of its own. `hostname` and `port` stay the
SSH endpoint; `enodix.ports` in the host data says, for each named port,
where to connect (`{"address": ..., "port": ...}`) or why that host cannot
be reached on it (`{"error": ...}`), for hosts behind jump hosts and hosts
reached directly alike (those get their own address and the port itself),
so the same task runs in a flat lab and behind a bastion:

```python
nr = InitNornir(inventory={"plugin": "EnodixInventory", "options": {
    "workspace": r"C:\Work\inventory", "tunnel_ports": [830, 443]}})

def facts(task):
    where = task.host["enodix"]["ports"][830]   # {"address", "port"} or {"error"}
    ...
```

No connection plugin is set up for the named ports: your task passes the
address and port to its library. NAPALM's `eos` driver, which uses eAPI
unless given `optional_args: {transport: ssh}`, is still pointed at
`hostname` and `port`, the SSH endpoint: run it over SSH, or give it
the 443 answer's address and port yourself. Nothing is probed when the tunnels open:
a port the device does not listen on fails when something connects to it,
that connection only, and the reason is logged as a `enodix` warning
(`port 830: ...`). Naming ports changes nothing about which hosts are
tunnelled: a host refused for its SSH port is refused for all of them.

**Host keys on SSH-based ports.** The app keeps the keys it trusts for a
device's SSH port; a device shows the same key on its NETCONF port, so the
known-hosts file of a tunnelled host lists those keys for every loopback
port of the host. `enodix.host_key_options(tunnel)` (with
`task.host["enodix"]["tunnel"]` or an entry of `Tunnels.hosts`) gives the
options that turn checking on, by library:

- **ncclient** (`manager.connect`): `hostkey_verify=True` and an
  `ssh_config` file written beside the known-hosts file whose only line is
  its `UserKnownHostsFile`: ncclient reads a known-hosts file of its
  choosing only through an SSH config (without one it checks against your
  own `~/.ssh/known_hosts`). A device that shows another key is refused
  with `SSHUnknownHostError`. One limit of ncclient's: it asks the device
  only for the type of the keys it knows, by the key's name, and an RSA
  key's name is `ssh-rsa` (SHA-1), which current devices no longer offer.
  So when the app trusts only an RSA key for a device that offers RSA only
  as `rsa-sha2-256`/`-512`, ncclient fails with "no acceptable host key"
  before any key is checked. Devices that also have an Ed25519 or ECDSA
  key, which the app prefers, are not affected.
- **scrapli-netconf** (`NetconfDriver`): `auth_strict_key=True` and
  `ssh_known_hosts_file`, as for scrapli. Tried with its `paramiko`
  transport; its default `system` transport hands the file to OpenSSH and
  was not tried.

Another key is refused by both. Other NETCONF stacks are not given a key
here: PyEZ (`junos-eznc`, which NAPALM's `junos` driver uses) does not
check the key through the tunnel, as noted above.

**TLS ports (HTTPS: eAPI, RESTCONF, NX-API, gNMI over TLS).** The app does
not check a device's certificate, here or anywhere: it carries the bytes.
Through a tunnel your script connects to `127.0.0.1`, so the certificate's
name cannot match, and device certificates are usually self-signed
anyway. The path is protected (SSH) up to the last jump host; from there
to the device, your script's own certificate check is the only one. Pin
the device's certificate or CA in your script (with `requests`, `verify=`
a CA file, and a name check that accepts `127.0.0.1` for that device), or
turn the check off knowingly (`verify=False`) as the example below does.

While a script runs, any local program can reach the devices' tunnelled
ports (as with `ssh -L`); it still needs their logins.

If a jump host goes down mid-run, the connections through it break at
once (the library reports a closed socket) and the reason is logged as a
`enodix` warning; the next connection tries the chain again.

Other limits: the libraries keep their own host-key handling for hosts
reached directly (they do not read the app's known hosts); telnet and
serial hosts are exported as they are (see `enodix.settings.protocol`),
with no telnet-specific driver names.

## Script jobs

The app runs a script of the workspace (any `.py` file in the workspace
folder or below it) as a job: "Run script…" on a folder, a saved filter or a
host. It uses the Python chosen in Preferences → Python, runs the script
once in the workspace folder, shows its output (stdout and stderr) as it
comes and keeps it with the other job results; exit status 0 counts as
succeeded, anything else as failed. Cancel ends the process.

The script finds the job in its environment:

| Variable             | Meaning                                                   |
| -------------------- | --------------------------------------------------------- |
| `ENODIX_WORKSPACE`   | The workspace folder: the default `workspace` everywhere. |
| `ENODIX_JOB_TARGETS` | A JSON file naming the job's hosts (`hosts`: host ids).   |
| `ENODIX_CLI`         | The headless binary beside the app.                       |

While `ENODIX_JOB_TARGETS` is set, the binary (and so this package) shows
only the job's hosts and hands over only their logins: a folder, filter or
host limit of the script's own narrows that further, a host outside the job
is refused, and so is any other workspace (the workspace's named secrets
stay readable). This guards against mistakes, not against the script: it
runs as you and could undo it. So in a job this is enough:

```python
from nornir import InitNornir

nr = InitNornir(inventory={"plugin": "EnodixInventory"})
```

The script has no console: a vault the running app has not unlocked is
refused rather than asked for. Pre-flight leaves out the hosts behind jump
hosts that cannot be tunnelled (a broken chain, a jump host with no stored
profile, telnet), with the reason.

## Fill in from devices

"Fill in from devices…" on a folder (or "Fill in from device…" on a host)
asks each device for its vendor, model, serial number and OS version. The
app runs this package's own script, `python -m enodix.enrich`, as a script
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
at once. The script writes what it found to the file `ENODIX_FACTS_FILE`
names; nothing is written to the workspace until you review it in the app
and apply: empty fields are filled, typed values are replaced only on the
hosts you tick. The OS version goes to the custom field `os_version`.

Verified with NAPALM 5.2 against cEOS 4.36, IOS-XE 17.3 (CSR 1000v) and
cJunos Evolved 25.4.

## Without Nornir

```python
import enodix

token = enodix.secret(r"C:\Work\inventory", "netbox-token")  # a named secret
export = enodix.inventory(r"C:\Work\inventory", filter="Edge")  # the binary's JSON
login = enodix.credentials(r"C:\Work\inventory", "Madrid/core-r1.yaml")
with enodix.tunnels(r"C:\Work\inventory", folder="Madrid") as opened:
    print(opened.hosts)  # host id -> address, port, known_hosts (or error)
```

`enodix.tunnels(workspace, ..., ports=[830, 443])` also carries those
device ports: each tunnelled host's entry gains `extra_ports`, device port
-> loopback port (without `ports` the entries are as above).

**Where do I connect?** `enodix.endpoint(host, port, tunnels)` (or
`tunnels.endpoint(host, port)`) takes a host of `enodix.inventory(...)`
and a device port and returns `(address, port)`: the tunnel's listener for
a host behind jump hosts, the host's own address and `port` for a host
reached directly (with or without tunnels). For a host behind jump hosts
that is not tunnelled, or a port that was not named, it raises
`enodix.Unreachable` (a `EnodixError`) with the reason. The host's own
SSH port is always answered.

NETCONF on 830 with ncclient, the device's key checked:

```python
import enodix
from ncclient import manager

ws = r"C:\Work\inventory"
hosts = enodix.inventory(ws, folder="Madrid")["hosts"]
with enodix.tunnels(ws, folder="Madrid", ports=[830]) as opened:
    for host in hosts:
        address, port = opened.endpoint(host, 830)
        login = enodix.credentials(ws, host["id"])
        tunnel = opened.hosts.get(host["id"])
        checks = enodix.host_key_options(tunnel)["ncclient"] if tunnel else {}
        with manager.connect(host=address, port=port, username=login["username"],
                             password=login["auth"]["password"], look_for_keys=False,
                             allow_agent=False, device_params={"name": "junos"},
                             **checks) as session:
            print(host["id"], session.get_config(source="running").data_xml[:200])
```

eAPI on 443 over HTTPS with requests (the certificate is not checked: see
"TLS ports" above):

```python
import enodix, requests

ws = r"C:\Work\inventory"
hosts = enodix.inventory(ws, filter="Arista")["hosts"]
with enodix.tunnels(ws, filter="Arista", ports=[443]) as opened:
    for host in hosts:
        address, port = opened.endpoint(host, 443)
        login = enodix.credentials(ws, host["id"])
        answer = requests.post(
            f"https://{address}:{port}/command-api",
            json={"jsonrpc": "2.0", "method": "runCmds", "id": 1,
                  "params": {"version": 1, "cmds": ["show version"]}},
            auth=(login["username"], login["auth"]["password"]), verify=False)
        print(host["id"], answer.json()["result"][0]["version"])
```

Errors are `enodix.EnodixError` (`CliNotFound`, `CliFailed` with the
binary's `status` and `reason`, or `Unreachable`). A binary of another
version than the package is a `enodix.VersionWarning` (a `UserWarning`),
given once; silence it with `warnings.filterwarnings("ignore",
category=enodix.VersionWarning)`.
