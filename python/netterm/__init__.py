"""The workspace of the app, for Python scripts.

Everything goes through the app's headless binary (``netterm-cli``), found
by :func:`find_cli`: the resolved inventory, a host's login from its
credential profile, and named secrets. Credentials travel only over the
binary's output pipe to this process; nothing is written to disk or put in
the environment.

Hosts behind the app's jump hosts are reached through loopback tunnels the
binary opens (:func:`tunnels`), for their SSH port and any other device
port a script names; :func:`endpoint` says where to connect for a port of
a host, tunnelled or not. The Nornir plugin opens them itself.

The Nornir inventory plugin is :class:`netterm.nornir.NettermInventory`
(registered as ``NettermInventory``); it needs Nornir, which this package
does not install.

Run by the app as a script job, a script finds its workspace in
``NETTERM_WORKSPACE`` (the default of every ``workspace`` argument here), and
the binary shows it only the job's hosts and hands over only their logins.
"""

from __future__ import annotations

import os
from os import PathLike, fspath
from typing import Any, Dict, Iterable, List, Optional, Union

from ._cli import (
    CliFailed,
    CliNotFound,
    NettermError,
    VersionWarning,
    default_location,
    find_cli,
    option,
    run,
    values,
)
from ._tunnel import Endpoint, Tunnels, Unreachable, endpoint, host_key_options, ports_of

try:
    from ._build import VERSION as __version__
except ImportError:  # the source tree
    __version__ = "0+source"

__all__ = [
    "CliFailed",
    "CliNotFound",
    "Endpoint",
    "NettermError",
    "Tunnels",
    "Unreachable",
    "VersionWarning",
    "credentials",
    "default_location",
    "endpoint",
    "job_workspace",
    "find_cli",
    "host_key_options",
    "inventory",
    "secret",
    "tunnels",
]

Folder = Union[str, "PathLike[str]"]

#: The variable the app sets for a script job: the workspace folder.
WORKSPACE_VARIABLE = "NETTERM_WORKSPACE"


def job_workspace() -> Optional[str]:
    """The workspace folder of the script job this script runs in (the app
    sets ``NETTERM_WORKSPACE``), or ``None`` when it was not started by one."""
    return os.environ.get(WORKSPACE_VARIABLE) or None


def _workspace(workspace: Optional[Folder]) -> str:
    if workspace is not None:
        return fspath(workspace)
    found = job_workspace()
    if found is None:
        raise NettermError(
            "no workspace given: pass the workspace folder (it is only known by itself "
            f"in a script job, which sets {WORKSPACE_VARIABLE})"
        )
    return found


def inventory(
    workspace: Optional[Folder] = None,
    *,
    folder: Optional[str] = None,
    filter: Optional[str] = None,
    hosts: Optional[Iterable[str]] = None,
) -> Dict[str, Any]:
    """The workspace's resolved inventory, as the binary exports it
    (``netterm-cli inventory``, format 1): every host with the settings
    that apply to it, its jump hosts, folders, tags and custom fields; no
    secrets. Limit it to one of: the hosts in ``folder`` and below, those
    of the saved filter ``filter``, or the host ids in ``hosts``. In a
    script job, only the job's hosts are ever given, and ``workspace``
    defaults to the job's."""
    return run(["inventory", *_workspace_and_limit(workspace, folder, filter, hosts)])


def _workspace_and_limit(
    workspace: Optional[Folder],
    folder: Optional[str],
    filter: Optional[str],
    hosts: Optional[Iterable[str]],
) -> List[str]:
    """``<workspace> [limit]`` for ``inventory`` and ``tunnel``; the limit
    options first and the workspace after ``--`` when its name starts with
    ``-``."""
    root = _workspace(workspace)
    limit = _limit(folder, filter, hosts)
    if root.startswith("-"):
        return [*limit, *values(root)]
    return [root, *limit]


def _limit(folder: Optional[str], filter: Optional[str], hosts: Optional[Iterable[str]]) -> List[str]:
    """The binary's limit options (``--folder=<value>`` for a value that
    starts with ``-``)."""
    args: List[str] = []
    host_ids = [hosts] if isinstance(hosts, str) else list(hosts or [])
    given = [name for name, value in (("folder", folder), ("filter", filter)) if value is not None]
    if host_ids:
        given.append("hosts")
    if len(given) > 1:
        raise ValueError(f"give only one of folder, filter and hosts, not {' and '.join(given)}")
    if folder is not None:
        args += option("--folder", folder)
    if filter is not None:
        args += option("--filter", filter)
    for host in host_ids:
        args += option("--host", host)
    return args


def tunnels(
    workspace: Optional[Folder] = None,
    *,
    folder: Optional[str] = None,
    filter: Optional[str] = None,
    hosts: Optional[Iterable[str]] = None,
    ports: Optional[Iterable[int]] = None,
) -> Tunnels:
    """Loopback tunnels to the hosts behind jump hosts among those
    :func:`inventory` gives with the same limit (``netterm-cli tunnel``):
    each jump chain is connected as the app would, with the jump hosts'
    stored logins and the host keys the app trusts, and nothing is asked.
    The result's ``hosts`` maps each such host id to where to connect
    instead (``address`` ``127.0.0.1``, ``port``) with a ``known_hosts``
    file of the device's trusted keys, or to an ``error`` saying why it is
    not tunnelled. ``ports`` are other device ports to carry for every
    such host (``[830, 443]``: NETCONF, an HTTPS API); each tunnelled
    host's entry then has ``extra_ports``, device port -> loopback port.
    :func:`endpoint` answers where to connect for any host and port. They
    stay open until closed or this process ends."""
    named = ports_of(ports)
    return Tunnels(_workspace_and_limit(workspace, folder, filter, hosts), named)


def credentials(workspace: Optional[Folder], host: str) -> Dict[str, Any]:
    """The login of the credential profile the host ``host`` (a host id)
    uses (``netterm-cli credentials``, format 1): ``username``, ``auth``
    (``{"kind": "password", "password"}``, ``{"kind": "private_key",
    "key_file", "passphrase"}`` or ``{"kind": "agent"}``) and
    ``enable_secret``. From the running app, else the store itself (the
    vault asks for its master password on the terminal). ``None`` is the
    script job's workspace; a job hands over only its hosts' logins."""
    return run(["credentials", *values(_workspace(workspace), host)])


def secret(workspace: Optional[Folder], name: str) -> str:
    """The value of the workspace's named secret ``name``, from the
    running app or the store, as :func:`credentials`."""
    return run(["secret", *values(_workspace(workspace), name)])["value"]
