"""Fill in from devices: the script the app runs (``python -m
netterm.enrich``) to fetch each host's vendor, model, serial number and OS
version with NAPALM.

The app starts it as a script job. ``NETTERM_JOB_TARGETS`` names the job's
hosts and the NAPALM driver of each (chosen by the app from the host's
platform definition); ``NETTERM_FACTS_FILE`` names the file the facts go
to, rewritten after every host::

    {"format": 1, "hosts": [
        {"id": "Site/r1.yaml", "facts": {"vendor": "Arista", "model": "cEOSLab",
                                          "serial_number": "...", "os_version": "4.36.0.1F"}},
        {"id": "Site/r2.yaml", "error": "why there are none"}
    ]}

Each host logs in with its own credential profile, handed over by the
app. Hosts behind jump hosts are reached through the binary's loopback
tunnels (:func:`netterm.tunnels`); NAPALM's drivers that log in through
Netmiko (``ios``, ``nxos_ssh``, and ``eos`` as run here, over SSH) then
check the device's host key against the keys the app trusts. Nothing is written to the workspace here: the app shows what the
devices report and fills in what the user applies. NAPALM is the user's
to install (the app's Python setup offers it); without it every host
fails, saying so.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any, Callable, Dict, List, Mapping, Optional

from . import Tunnels, credentials, inventory, tunnels
from ._tunnel import host_key_options

#: The variable naming the file the facts go to.
FACTS_VARIABLE = "NETTERM_FACTS_FILE"
#: The variable naming the job's targets file.
TARGETS_VARIABLE = "NETTERM_JOB_TARGETS"
#: The version of the facts file's format.
FORMAT = 1
#: The facts NAPALM's ``get_facts`` gives that the app fills in.
FACTS = ("vendor", "model", "serial_number", "os_version")
#: How many devices are asked at once.
PARALLEL = 10
#: How often the facts file is tried again when it cannot be replaced (50 ms
#: apart).
REPLACE_TRIES = 40

DriverFinder = Callable[[str], Any]


#: The NAPALM drivers that log in through Netmiko, which takes a known-hosts
#: file: a tunnelled host's key is checked with these only. ``eos`` is one
#: of them because it is given the SSH transport here (not eAPI).
NETMIKO_DRIVERS = ("ios", "nxos_ssh", "eos")


def driver_arguments(
    host: Mapping[str, Any],
    login: Mapping[str, Any],
    driver: str,
    tunnel: Optional[Mapping[str, Any]] = None,
) -> Dict[str, Any]:
    """The keyword arguments of NAPALM's driver class ``driver`` for the
    exported ``host`` and its ``login`` (``netterm-cli credentials``),
    through ``tunnel`` (an entry of :attr:`netterm.Tunnels.hosts`) if the
    host is behind jump hosts."""
    settings = host.get("settings") or {}
    optional: Dict[str, Any] = {}
    port = tunnel["port"] if tunnel else settings.get("port")
    # Every driver goes to the host's SSH port, Junos too: NETCONF over
    # SSH's own port works where port 830 is not open.
    if port:
        optional["port"] = port
    if driver == "eos":
        # NAPALM's default is eAPI over HTTPS, which a device may not
        # serve; every device the app knows is reached over SSH.
        optional["transport"] = "ssh"
    auth = login.get("auth") or {}
    password = ""
    kind = auth.get("kind")
    if kind == "password":
        password = auth.get("password") or ""
    elif kind == "private_key":
        optional.update(use_keys=True, key_file=auth["key_file"])
        if auth.get("passphrase"):
            optional["passphrase"] = auth["passphrase"]
    elif kind == "agent":
        optional.update(use_keys=True, allow_agent=True)
    if login.get("enable_secret"):
        optional["secret"] = login["enable_secret"]
    if tunnel and driver in NETMIKO_DRIVERS:
        optional.update(host_key_options(dict(tunnel))["napalm"])
    return {
        "hostname": tunnel["address"] if tunnel else host["address"],
        "username": login.get("username") or "",
        "password": password,
        "timeout": settings.get("connect_timeout") or 60,
        "optional_args": optional,
    }


def facts_of(reported: Mapping[str, Any]) -> Dict[str, str]:
    """The facts the app fills in from NAPALM's ``get_facts``, as text,
    leaving out blank ones."""
    facts = {}
    for key in FACTS:
        value = reported.get(key)
        text = "" if value is None else str(value).strip()
        if text:
            facts[key] = text
    return facts


def _reason(error: BaseException) -> str:
    text = str(error).strip().splitlines()
    first = text[0] if text else ""
    name = type(error).__name__
    return f"{name}: {first}" if first else name


def fetch(get_driver: DriverFinder, driver: str, arguments: Dict[str, Any]) -> Dict[str, str]:
    """The facts of one device."""
    device = get_driver(driver)(**arguments)
    device.open()
    try:
        return facts_of(device.get_facts())
    finally:
        try:
            device.close()
        except Exception:  # the facts are in; a failing close changes nothing
            pass


def _write(path: Path, hosts: List[Dict[str, Any]]) -> None:
    """Writes the facts file whole (a temporary file renamed over it), so
    that the app never reads it half written."""
    text = json.dumps({"format": FORMAT, "hosts": hosts}, indent=2)
    handle, temporary = tempfile.mkstemp(prefix=".facts-", dir=path.parent)
    with os.fdopen(handle, "w", encoding="utf-8") as out:
        out.write(text)
    # On Windows a file cannot be replaced while something reads it (a
    # virus scanner, an indexer): try again for a moment rather than lose
    # every host's facts (#119).
    for attempt in range(REPLACE_TRIES):
        try:
            os.replace(temporary, path)
            return
        except PermissionError:
            if attempt + 1 == REPLACE_TRIES:
                os.remove(temporary)
                raise
            time.sleep(0.05)


def collect(
    targets: Mapping[str, Any],
    facts_file: Path,
    get_driver: Optional[DriverFinder] = None,
    out: Callable[[str], None] = print,
) -> List[Dict[str, Any]]:
    """Fetches the facts of every host in ``targets`` (the job's targets
    file), writing ``facts_file`` after each; returns its entries.
    ``get_driver`` is NAPALM's ``get_network_driver`` (imported if not
    given)."""
    ids: List[str] = list(targets.get("hosts") or [])
    drivers: Dict[str, str] = dict(targets.get("napalm_drivers") or {})
    entries: Dict[str, Dict[str, Any]] = {}

    def save() -> List[Dict[str, Any]]:
        listed = [entries[host] for host in ids if host in entries]
        _write(facts_file, listed)
        return listed

    def fail_all(reason: str) -> List[Dict[str, Any]]:
        for host in ids:
            entries[host] = {"id": host, "error": reason}
            out(f"{host}: {reason}")
        return save()

    save()
    if get_driver is None:
        try:
            from napalm import get_network_driver
        except ImportError as error:
            return fail_all(
                f"NAPALM is not installed in this Python ({sys.executable}): {error}. "
                "Install it with the app's Python setup or with pip install napalm"
            )
        get_driver = get_network_driver
    try:
        exported = {host["id"]: host for host in inventory(hosts=ids)["hosts"]} if ids else {}
    except Exception as error:
        return fail_all(f"the inventory could not be read: {_reason(error)}")
    behind = [host for host, value in exported.items() if value["jump_hosts"] or value["jump_chain_error"]]
    tunnelled: Dict[str, Dict[str, Any]] = {}
    opened: Optional[Tunnels] = None
    if behind:
        try:
            opened = tunnels(hosts=behind)
            tunnelled = dict(opened.hosts)
        except Exception as error:
            reason = f"the tunnels to the hosts behind jump hosts could not be opened: {_reason(error)}"
            tunnelled = {host: {"error": reason} for host in behind}

    def one(host_id: str) -> Dict[str, Any]:
        driver = drivers.get(host_id)
        host = exported.get(host_id)
        if not driver:
            return {"id": host_id, "error": "the app named no NAPALM driver for it"}
        if host is None:
            return {"id": host_id, "error": "it is not in the inventory"}
        tunnel = tunnelled.get(host_id)
        if tunnel is not None and "error" in tunnel:
            return {"id": host_id, "error": f"it cannot be reached through its jump hosts: {tunnel['error']}"}
        try:
            login = credentials(None, host_id)
        except Exception as error:
            return {"id": host_id, "error": f"its login could not be had: {_reason(error)}"}
        try:
            facts = fetch(get_driver, driver, driver_arguments(host, login, driver, tunnel))
        except Exception as error:
            return {"id": host_id, "error": f"NAPALM ({driver}) failed: {_reason(error)}"}
        return {"id": host_id, "facts": facts}

    try:
        with ThreadPoolExecutor(max_workers=PARALLEL) as pool:
            running = {pool.submit(one, host): host for host in ids}
            for done in as_completed(running):
                entry = done.result()
                entries[entry["id"]] = entry
                if "facts" in entry:
                    shown = ", ".join(f"{key} {value}" for key, value in entry["facts"].items())
                    out(f"{entry['id']}: {shown or 'no facts reported'}")
                else:
                    out(f"{entry['id']}: {entry['error']}")
                save()
    finally:
        if opened is not None:
            opened.close()
    return save()


def main() -> int:
    targets_file = os.environ.get(TARGETS_VARIABLE)
    facts_file = os.environ.get(FACTS_VARIABLE)
    if not targets_file or not facts_file:
        print(
            f"run by the app's Fill in from devices, which sets {TARGETS_VARIABLE} and {FACTS_VARIABLE}",
            file=sys.stderr,
        )
        return 2
    with open(targets_file, encoding="utf-8") as source:
        targets = json.load(source)
    entries = collect(targets, Path(facts_file))
    found = sum(1 for entry in entries if "facts" in entry)
    print(f"facts of {found} of {len(entries)} hosts")
    return 0


if __name__ == "__main__":
    sys.exit(main())
