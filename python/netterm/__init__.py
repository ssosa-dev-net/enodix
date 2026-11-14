"""The workspace of the app, for Python scripts.

Everything goes through the app's headless binary (``netterm-cli``), found
by :func:`find_cli`: the resolved inventory, a host's login from its
credential profile, and named secrets. Credentials travel only over the
binary's output pipe to this process; nothing is written to disk or put in
the environment.

The Nornir inventory plugin is :class:`netterm.nornir.NettermInventory`
(registered as ``NettermInventory``); it needs Nornir, which this package
does not install.
"""

from __future__ import annotations

from os import PathLike, fspath
from typing import Any, Dict, Iterable, Optional, Union

from ._cli import CliFailed, CliNotFound, NettermError, default_location, find_cli, run

try:
    from ._build import VERSION as __version__
except ImportError:  # the source tree
    __version__ = "0+source"

__all__ = [
    "CliFailed",
    "CliNotFound",
    "NettermError",
    "credentials",
    "default_location",
    "find_cli",
    "inventory",
    "secret",
]

Folder = Union[str, "PathLike[str]"]


def inventory(
    workspace: Folder,
    *,
    folder: Optional[str] = None,
    filter: Optional[str] = None,
    hosts: Optional[Iterable[str]] = None,
) -> Dict[str, Any]:
    """The workspace's resolved inventory, as the binary exports it
    (``netterm-cli inventory``, format 1): every host with the settings
    that apply to it, its jump hosts, folders, tags and custom fields; no
    secrets. Limit it to one of: the hosts in ``folder`` and below, those
    of the saved filter ``filter``, or the host ids in ``hosts``."""
    args = ["inventory", fspath(workspace)]
    host_ids = [hosts] if isinstance(hosts, str) else list(hosts or [])
    given = [name for name, value in (("folder", folder), ("filter", filter)) if value is not None]
    if host_ids:
        given.append("hosts")
    if len(given) > 1:
        raise ValueError(f"give only one of folder, filter and hosts, not {' and '.join(given)}")
    if folder is not None:
        args += ["--folder", folder]
    if filter is not None:
        args += ["--filter", filter]
    for host in host_ids:
        args += ["--host", host]
    return run(args)


def credentials(workspace: Folder, host: str) -> Dict[str, Any]:
    """The login of the credential profile the host ``host`` (a host id)
    uses (``netterm-cli credentials``, format 1): ``username``, ``auth``
    (``{"kind": "password", "password"}``, ``{"kind": "private_key",
    "key_file", "passphrase"}`` or ``{"kind": "agent"}``) and
    ``enable_secret``. From the running app, else the store itself (the
    vault asks for its master password on the terminal)."""
    return run(["credentials", fspath(workspace), host])


def secret(workspace: Folder, name: str) -> str:
    """The value of the workspace's named secret ``name``, from the
    running app or the store, as :func:`credentials`."""
    return run(["secret", fspath(workspace), name])["value"]
