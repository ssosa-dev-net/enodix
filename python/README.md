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

The package finds `netterm-cli` through the `NETTERM_CLI` variable if set
(the app sets it for scripts it starts), else at the installer's location
(`%LOCALAPPDATA%\netterm\netterm-cli.exe`), else on `PATH`. If it is in none
of them, it says where it looked: set `NETTERM_CLI` to the binary's path (for
example inside the portable folder).

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
  `jump_chain_error`, `behind_jump_host`, `withheld` and `custom` (all the
  custom fields; a custom field with a built-in field's name is only here).
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

The Python libraries make their own SSH connections and cannot go through
the app's jump hosts yet. Such hosts are still in the inventory, marked
`netterm.behind_jump_host` (with `netterm.jump_hosts`, or
`netterm.jump_chain_error` when the chain is broken), and a warning names
them when the inventory loads. Leave them out with
`nr.filter(F(netterm__behind_jump_host=False))`. How scripts will reach them
is not decided yet.

Other limits: the libraries keep their own host-key handling (they do not
read the app's known hosts); telnet and serial hosts are exported as they
are (see `netterm.settings.protocol`), with no telnet-specific driver
names.

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
refused rather than asked for.

## Fill in from devices

"Fill in from devices…" on a folder (or "Fill in from device…" on a host)
asks each device for its vendor, model, serial number and OS version. The
app runs this package's own script, `python -m netterm.enrich`, as a script
job with the Python chosen in Preferences; it needs
[NAPALM](https://napalm.readthedocs.io/) in that Python (`pip install
napalm`, or tick NAPALM in the app's Python setup). Pre-flight keeps the
hosts reached over SSH with a stored credential profile and no jump host
whose platform definition answers to a NAPALM driver (`ios`, `eos`,
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
```

Errors are `netterm.NettermError` (`CliNotFound`, or `CliFailed` with the
binary's `status` and `reason`).
