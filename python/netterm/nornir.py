"""The workspace as a Nornir inventory.

::

    from nornir import InitNornir

    nr = InitNornir(inventory={
        "plugin": "NettermInventory",
        "options": {"workspace": r"C:\\Work\\inventory", "folder": "Europe/Madrid"},
    })

In a script job of the app, ``InitNornir(inventory={"plugin":
"NettermInventory"})`` is enough: the workspace is the job's and the
inventory holds only the job's hosts.

Options: ``workspace`` (the folder; in a script job, the job's by default);
at most one of ``folder``
(that folder and below), ``filter`` (a saved filter's name) and ``hosts``
(host ids); ``credentials`` (default ``True``: hand over each host's login
from its credential profile; ``False`` reads no store).

The mapping:

- Each host is a Nornir host named by its host id (``Madrid/core-r1.yaml``,
  unique in the workspace), with ``hostname`` = its management address,
  ``port`` and ``platform`` as resolved (the platform as written, so write
  the name your library expects, such as ``arista_eos``).
- Each folder is a group named by its path (``Madrid/Lab``), whose parent
  group is the folder above; a host is in its own folder's group (hosts at
  the root are in none). Group data: ``description`` and the folder's own
  ``tags``.
- Host data: the custom fields, the built-in fields that are set
  (``vendor``, ``model``, ``serial_number``, ``site``, ``building``,
  ``room``, ``rack``, ``rack_unit``, ``role``, ``description``; a custom
  field of the same name is only in ``netterm.custom``), ``tags`` (every
  tag that applies, inherited ones included), and ``netterm``: ``id``,
  ``name``, ``folder``, ``folders``, ``settings`` (every resolved setting),
  ``login`` (``password``, ``private_key``, ``agent`` or ``None``),
  ``jump_hosts`` (ids, in connect order), ``jump_chain_error``,
  ``behind_jump_host``, ``withheld`` (secret-named custom fields left out)
  and ``custom``.
- Logins: a password profile sets ``username`` and ``password``; a key
  profile ``username`` and, in the ``netmiko``, ``scrapli`` and ``napalm``
  connection options, the key file and its passphrase; an agent profile
  only ``username`` (the libraries use the SSH agent themselves). An
  enable secret goes to those options too (``secret``,
  ``auth_secondary``).

Hosts reached through jump hosts are marked (``netterm.behind_jump_host``)
and a warning names them: the Python libraries make their own connections
and cannot go through the app's jump hosts yet.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, Iterable, List, Mapping, Optional, Union

from nornir.core.inventory import (
    ConnectionOptions,
    Defaults,
    Group,
    Groups,
    Host,
    Hosts,
    Inventory,
    ParentGroups,
)

from . import Folder, NettermError, credentials, inventory

__all__ = ["NettermInventory", "build_inventory"]

LOG = logging.getLogger("netterm")

#: Built-in host fields copied into the host data when set.
BUILT_IN_FIELDS = (
    "vendor",
    "model",
    "serial_number",
    "site",
    "building",
    "room",
    "rack",
    "rack_unit",
    "role",
    "description",
)
#: Host data keys of the package's own, which no field replaces.
RESERVED = ("tags", "netterm")


class NettermInventory:
    """Nornir inventory plugin: the workspace's hosts and folders, with the
    logins of their credential profiles (see the module documentation)."""

    def __init__(
        self,
        workspace: Optional[Folder] = None,
        folder: Optional[str] = None,
        filter: Optional[str] = None,
        hosts: Optional[Union[str, Iterable[str]]] = None,
        credentials: bool = True,
    ) -> None:
        self.workspace = workspace
        self.folder = folder
        self.filter = filter
        self.hosts = hosts
        self.credentials = credentials

    def load(self) -> Inventory:
        exported = inventory(
            self.workspace,
            folder=self.folder,
            filter=self.filter,
            hosts=self.hosts,
        )
        logins = _logins(exported) if self.credentials else {}
        return build_inventory(exported, logins)


def _logins(exported: Mapping[str, Any]) -> Dict[str, Dict[str, Any]]:
    """Each host's login (by host id), asked once per credential profile."""
    root = exported["workspace"]["root"]
    by_profile: Dict[str, Dict[str, Any]] = {}
    logins: Dict[str, Dict[str, Any]] = {}
    for host in exported["hosts"]:
        profile = host["settings"]["credential_profile"]
        if not profile:
            continue
        key = profile.lower()
        if key not in by_profile:
            try:
                by_profile[key] = credentials(root, host["id"])
            except NettermError as error:
                raise NettermError(
                    f"cannot get the login of {host['id']} (credential profile {profile}): "
                    f"{error}. Set the profile up, limit the inventory to other hosts, "
                    "or load it with credentials=False"
                ) from error
        logins[host["id"]] = by_profile[key]
    return logins


def build_inventory(
    exported: Mapping[str, Any],
    logins: Optional[Mapping[str, Mapping[str, Any]]] = None,
) -> Inventory:
    """The Nornir inventory of an export (:func:`netterm.inventory`) with
    the given logins (:func:`netterm.credentials`, by host id)."""
    logins = logins or {}
    defaults = Defaults()
    groups = Groups()
    for folder in exported["folders"]:
        data: Dict[str, Any] = {"tags": list(folder["tags"])}
        if folder.get("description") is not None:
            data["description"] = folder["description"]
        groups[folder["path"]] = Group(name=folder["path"], data=data, defaults=defaults)
    for path, group in groups.items():
        parent = path.rpartition("/")[0]
        if parent in groups:
            group.groups = ParentGroups([groups[parent]])

    hosts = Hosts()
    behind: List[str] = []
    for exported_host in exported["hosts"]:
        host = _host(exported_host, logins.get(exported_host["id"]), groups, defaults)
        hosts[host.name] = host
        if host.data["netterm"]["behind_jump_host"]:
            behind.append(host.name)
    for problem in exported.get("problems", []):
        LOG.warning("workspace problem: %s: %s", problem["file"], problem["message"])
    if behind:
        LOG.warning(
            "%d host(s) are reached through jump hosts, which the Python libraries cannot "
            "use yet (marked netterm.behind_jump_host): %s",
            len(behind),
            ", ".join(behind),
        )
    return Inventory(hosts=hosts, groups=groups, defaults=defaults)


def _host(
    exported: Mapping[str, Any],
    login: Optional[Mapping[str, Any]],
    groups: Groups,
    defaults: Defaults,
) -> Host:
    settings = exported["settings"]
    custom = dict(exported["custom"])
    jump_hosts = [jump["id"] for jump in exported["jump_hosts"]]
    data: Dict[str, Any] = {key: value for key, value in custom.items() if key not in RESERVED}
    for field in BUILT_IN_FIELDS:
        if exported.get(field) is not None:
            data[field] = exported[field]
    data["tags"] = list(exported["tags"])
    data["netterm"] = {
        "id": exported["id"],
        "name": exported["name"],
        "folder": exported["folder"],
        "folders": list(exported["folders"]),
        "settings": settings,
        "login": login["auth"]["kind"] if login else None,
        "jump_hosts": jump_hosts,
        "jump_chain_error": exported["jump_chain_error"],
        "behind_jump_host": bool(jump_hosts or exported["jump_chain_error"]),
        "withheld": list(exported["withheld"]),
        "custom": custom,
    }
    username, password, options = _login(login)
    folder = exported["folder"]
    return Host(
        name=exported["id"],
        hostname=exported["address"],
        username=username,
        password=password,
        port=settings["port"],
        platform=settings["platform"],
        groups=ParentGroups([groups[folder]] if folder in groups else []),
        data=data,
        connection_options=options,
        defaults=defaults,
    )


def _login(
    login: Optional[Mapping[str, Any]],
) -> "tuple[Optional[str], Optional[str], Dict[str, ConnectionOptions]]":
    """Username, password and connection options for a login."""
    if not login:
        return None, None, {}
    auth = login["auth"]
    netmiko: Dict[str, Any] = {}
    scrapli: Dict[str, Any] = {}
    napalm: Dict[str, Any] = {}
    password = None
    if auth["kind"] == "password":
        password = auth["password"]
    elif auth["kind"] == "private_key":
        netmiko.update(use_keys=True, key_file=auth["key_file"])
        scrapli["auth_private_key"] = auth["key_file"]
        napalm.update(use_keys=True, key_file=auth["key_file"])
        if auth.get("passphrase") is not None:
            netmiko["passphrase"] = auth["passphrase"]
            scrapli["auth_private_key_passphrase"] = auth["passphrase"]
            napalm["passphrase"] = auth["passphrase"]
    elif auth["kind"] == "agent":
        netmiko.update(use_keys=True, allow_agent=True)
        napalm.update(use_keys=True, allow_agent=True)
    if login.get("enable_secret") is not None:
        netmiko["secret"] = login["enable_secret"]
        scrapli["auth_secondary"] = login["enable_secret"]
        napalm["secret"] = login["enable_secret"]
    options: Dict[str, ConnectionOptions] = {}
    if netmiko:
        options["netmiko"] = ConnectionOptions(extras=netmiko)
    if scrapli:
        options["scrapli"] = ConnectionOptions(extras=scrapli)
    if napalm:
        options["napalm"] = ConnectionOptions(extras={"optional_args": napalm})
    return login["username"], password, options
