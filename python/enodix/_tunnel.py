"""Loopback tunnels to the hosts behind jump hosts, opened by the headless
binary (``enodix-cli tunnel``) and kept open while this process runs.

The binary connects each jump chain as the app would (the jump hosts'
stored logins, the host keys the app trusts) and listens on ``127.0.0.1``,
one port per host, and one more per extra device port the script named
(NETCONF on 830, an API on 443, ...). A library connects there instead of
to the host (:func:`endpoint` says where), and checks the device's own host
key against a known-hosts file written here from the keys the app trusts
(``127.0.0.1 <key>`` and ``[127.0.0.1]:<port> <key>`` for every loopback
port of the host, one file per host, as scrapli's paramiko transport
ignores the port). The tunnels end when :meth:`Tunnels.close` is called,
when this process exits, or when it is killed: the binary serves until its
standard input, held here, ends.
"""

from __future__ import annotations

import atexit
import json
import logging
import shutil
import subprocess
import tempfile
import threading
from pathlib import Path
from typing import Any, Dict, Iterable, List, Mapping, NamedTuple, Optional, Sequence, Tuple

from ._cli import (
    CLI_NAME,
    END_OF_OPTIONS,
    FORMAT,
    CliFailed,
    EnodixError,
    compare_version,
    find_cli,
)

LOG = logging.getLogger("enodix")

#: Every tunnel process this one started and has not closed.
_OPEN: List["Tunnels"] = []
_OPEN_LOCK = threading.Lock()


class Unreachable(EnodixError):
    """:func:`endpoint` has no address for that port of that host; the
    message says why."""


class Endpoint(NamedTuple):
    """Where to connect for a port of a host."""

    address: str
    port: int


def ports_of(ports: Optional[Iterable[int]]) -> Tuple[int, ...]:
    """The device ports, checked: each an ``int`` from 1 to 65535."""
    if ports is None:
        return ()
    if isinstance(ports, (str, bytes, int)):
        raise ValueError(f"ports must be a list of port numbers, not {ports!r}")
    checked = []
    for port in ports:
        if isinstance(port, bool) or not isinstance(port, int) or not 1 <= port <= 65535:
            raise ValueError(f"{port!r} is not a port: give whole numbers from 1 to 65535")
        checked.append(port)
    return tuple(checked)


class Tunnels:
    """A running ``enodix-cli tunnel``: ``hosts`` maps each host behind
    jump hosts (host id) to ``{"address", "port", "host_keys",
    "known_hosts"}`` when it is tunnelled, with ``"extra_ports"`` (device
    port -> loopback port) when ports were named, or to ``{"error"}``
    saying why not; ``ports`` are the device ports named. Use as a context
    manager, or call :meth:`close`; it is closed at exit anyway."""

    def __init__(self, args: Sequence[str], ports: Sequence[int] = ()) -> None:
        self.ports: Tuple[int, ...] = tuple(ports)
        named = [part for port in self.ports for part in ("--port", str(port))]
        args = list(args)
        # Options go before the end of the options, if the values need one.
        end = args.index(END_OF_OPTIONS) if END_OF_OPTIONS in args else len(args)
        cli = find_cli()
        compare_version(cli)
        try:
            self._process = subprocess.Popen(
                [str(cli), "tunnel", *args[:end], *named, *args[end:]],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        except OSError as error:
            raise EnodixError(f"cannot start {cli}: {error}") from error
        self.hosts: Dict[str, Dict[str, Any]] = {}
        self._folder: Optional[Path] = None
        assert self._process.stdout is not None and self._process.stderr is not None
        # Read from the start, so that the binary never waits on a full
        # stderr pipe; kept until the first line is in, logged after.
        self._early: Optional[List[str]] = []
        self._early_lock = threading.Lock()
        reader = threading.Thread(target=self._report, name="enodix tunnel stderr", daemon=True)
        reader.start()
        line = self._process.stdout.readline()
        if not line:
            status = self._process.wait()
            reader.join(10)
            reason = "\n".join(self._early or []).strip()
            prefix = f"{CLI_NAME}: error: "
            said = " ".join(part.removeprefix(prefix) for part in reason.splitlines())
            raise CliFailed(said or f"it exited with status {status}", status, reason)
        with self._early_lock:
            early, self._early = self._early or [], None
        for text in early:
            LOG.warning("tunnel: %s", text.removeprefix(f"{CLI_NAME}: tunnel: "))
        with _OPEN_LOCK:
            _OPEN.append(self)
        try:
            answer = json.loads(line.decode("utf-8"))
            if not isinstance(answer, dict) or answer.get("format") != FORMAT:
                raise EnodixError(
                    f"{cli} speaks another format than {FORMAT}: install the package that "
                    "ships with that version of the app"
                )
            self._take(answer["tunnels"])
        except Exception:
            self.close()
            raise

    def _take(self, tunnels: List[Dict[str, Any]]) -> None:
        for entry in tunnels:
            host = entry["host"]
            if entry.get("status") != "listening":
                self.hosts[host] = {"error": entry.get("reason") or "it is not tunnelled"}
                continue
            if self._folder is None:
                self._folder = Path(tempfile.mkdtemp(prefix="enodix-tunnels-"))
            address, port = entry["address"], entry["port"]
            extra = {int(device): loopback for device, loopback in (entry.get("extra_ports") or {}).items()}
            known_hosts = self._folder / f"known_hosts_{len(self.hosts)}"
            lines = []
            for key in entry["host_keys"]:
                lines.append(f"{address} {key}\n")
                for loopback in (port, *extra.values()):
                    lines.append(f"[{address}]:{loopback} {key}\n")
            known_hosts.write_text("".join(lines), encoding="utf-8")
            # For ncclient, which reads a known-hosts file of its choosing
            # only through an SSH config (else the user's ~/.ssh/known_hosts).
            ssh_config_for(str(known_hosts)).write_text(
                f'Host *\n  UserKnownHostsFile "{known_hosts}"\n', encoding="utf-8"
            )
            self.hosts[host] = {
                "address": address,
                "port": port,
                "host_keys": list(entry["host_keys"]),
                "known_hosts": str(known_hosts),
            }
            if extra:
                self.hosts[host]["extra_ports"] = extra

    def endpoint(self, host: Mapping[str, Any], port: int) -> Endpoint:
        """Where to connect for the device port ``port`` of ``host`` (a host
        of :func:`enodix.inventory`): see :func:`endpoint`."""
        return endpoint(host, port, self)

    def _report(self) -> None:
        """Passes on what the binary reports while it serves."""
        stderr = self._process.stderr
        assert stderr is not None
        prefix = f"{CLI_NAME}: tunnel: "
        with stderr:
            for raw in iter(stderr.readline, b""):
                text = raw.decode("utf-8", errors="replace").strip()
                if not text:
                    continue
                with self._early_lock:
                    if self._early is not None:
                        self._early.append(text)
                        continue
                LOG.warning("tunnel: %s", text.removeprefix(prefix))

    def close(self) -> None:
        """Ends the tunnels (and every connection through them) and removes
        the known-hosts files."""
        with _OPEN_LOCK:
            if self in _OPEN:
                _OPEN.remove(self)
        process = self._process
        if process.poll() is None:
            try:
                if process.stdin is not None:
                    process.stdin.close()
                process.wait(10)
            except (OSError, subprocess.TimeoutExpired):
                process.kill()
                process.wait()
        if process.stdout is not None:
            process.stdout.close()
        if self._folder is not None:
            shutil.rmtree(self._folder, ignore_errors=True)
            self._folder = None

    def __enter__(self) -> "Tunnels":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


@atexit.register
def _close_all() -> None:
    with _OPEN_LOCK:
        running = list(_OPEN)
    for tunnels in running:
        tunnels.close()


#: Why a telnet host with a jump host is not tunnelled (the binary's words).
TELNET_REASON = "it is reached over telnet through a jump host, and only SSH is tunnelled"


def behind(host: Mapping[str, Any]) -> bool:
    """Whether the exported host is reached through jump hosts (a broken
    chain included)."""
    return bool(host["jump_hosts"] or host["jump_chain_error"])


def _behind_any(host: Mapping[str, Any]) -> bool:
    """:func:`behind`, or a telnet host with a jump host (the export gives
    those no chain: only SSH is tunnelled)."""
    settings = host.get("settings") or {}
    return behind(host) or (settings.get("protocol") == "telnet" and bool(settings.get("jump_host")))


def answer(
    host: Mapping[str, Any],
    port: int,
    tunnel: Optional[Mapping[str, Any]],
    missing: str,
) -> Endpoint:
    """:func:`endpoint`, given the host's entry in :attr:`Tunnels.hosts`
    (``None`` when it has none) and why a host behind jump hosts without
    an entry is not tunnelled."""
    name = host["id"]
    settings = host.get("settings") or {}
    if tunnel is None and not _behind_any(host):
        if settings.get("protocol") == "serial":
            raise Unreachable(f"{name} is a serial host: it has no network address")
        return Endpoint(host["address"], port)
    if tunnel is None:
        why = missing if behind(host) else TELNET_REASON
        raise Unreachable(f"{name} is behind jump hosts and not tunnelled: {why}")
    if "error" in tunnel:
        raise Unreachable(f"{name} is behind jump hosts and not tunnelled: {tunnel['error']}")
    if port == settings.get("port"):
        return Endpoint(tunnel["address"], tunnel["port"])
    loopback = (tunnel.get("extra_ports") or {}).get(port)
    if loopback is None:
        raise Unreachable(
            f"{name} is behind jump hosts and port {port} was not named when the tunnels "
            "were opened (ports=...)"
        )
    return Endpoint(tunnel["address"], loopback)


def endpoint(host: Mapping[str, Any], port: int, tunnels: Optional[Tunnels] = None) -> Endpoint:
    """Where to connect for the device port ``port`` of ``host`` (a host of
    :func:`enodix.inventory`), as ``(address, port)``: for a host behind
    jump hosts, the loopback listener of its tunnel for that port (opened
    with ``tunnels``, with the port named, or the host's own SSH port); for
    any other host, its own address and ``port``. Raises
    :class:`Unreachable`, saying why, for a host behind jump hosts that is
    not tunnelled and for a port that was not named."""
    (port,) = ports_of([port])
    if tunnels is None:
        return answer(host, port, None, "no tunnels were opened (enodix.tunnels(...))")
    return answer(
        host,
        port,
        tunnels.hosts.get(host["id"]),
        "it is not among the hosts the tunnels were opened for",
    )


#: The folder of the known-hosts files written for hosts reached directly
#: (:func:`direct_host_keys`), made at the first and removed at exit.
_DIRECT: Dict[str, Any] = {"folder": None, "count": 0}
_DIRECT_LOCK = threading.Lock()


@atexit.register
def _remove_direct() -> None:
    with _DIRECT_LOCK:
        folder, _DIRECT["folder"] = _DIRECT["folder"], None
    if folder is not None:
        shutil.rmtree(folder, ignore_errors=True)


def direct_host_keys(host: Mapping[str, Any]) -> Optional[Dict[str, Any]]:
    """For an exported host reached directly (not :func:`behind` jump
    hosts): the same entry as a tunnelled host's (``address``, ``port``,
    ``host_keys`` and a ``known_hosts`` file naming only the keys the app
    trusts for it, with an SSH config beside it), so that
    :func:`host_key_options` makes each library check its key as the app
    does. ``None`` when the app trusts no key for it (never connected to in
    the app, or serial): a library left to itself would accept whatever
    key answered. The files are in a folder only this user can read
    (``mkdtemp``), removed when Python exits."""
    keys = list(host.get("host_keys") or [])
    port = (host.get("settings") or {}).get("port")
    if not keys or not port:
        return None
    address = host["address"]
    with _DIRECT_LOCK:
        if _DIRECT["folder"] is None:
            _DIRECT["folder"] = Path(tempfile.mkdtemp(prefix="enodix-host-keys-"))
        folder: Path = _DIRECT["folder"]
        _DIRECT["count"] += 1
        known_hosts = folder / f"known_hosts_{_DIRECT['count']}"
    name = address.lower()
    lines = [f"{name} {key}\n" if port == 22 else f"[{name}]:{port} {key}\n" for key in keys]
    known_hosts.write_text("".join(lines), encoding="utf-8")
    ssh_config_for(str(known_hosts)).write_text(
        f'Host *\n  UserKnownHostsFile "{known_hosts}"\n', encoding="utf-8"
    )
    return {"address": address, "port": port, "host_keys": keys, "known_hosts": str(known_hosts)}


def ssh_config_for(known_hosts: str) -> Path:
    """The SSH config written beside a tunnel's known-hosts file: it names
    only that file (for ncclient)."""
    path = Path(known_hosts)
    return path.with_name(f"{path.name}_ssh_config")


def host_key_options(tunnel: Mapping[str, Any]) -> Dict[str, Dict[str, Any]]:
    """For a tunnelled host (an entry of :attr:`Tunnels.hosts`, or the
    Nornir host data's ``enodix.tunnel``): the options that make each
    library check the device's host key against the keys the app trusts,
    on the SSH port and on every extra port. By library: ``netmiko``,
    ``scrapli``, ``napalm``'s ``optional_args`` (which its Netmiko-based
    drivers pass on), ``ncclient`` (for ``manager.connect``:
    ``hostkey_verify`` and an ``ssh_config`` that names only the
    known-hosts file) and ``scrapli_netconf`` (as scrapli)."""
    known_hosts = tunnel["known_hosts"]
    netmiko = {
        "ssh_strict": True,
        "system_host_keys": False,
        "alt_host_keys": True,
        "alt_key_file": known_hosts,
    }
    scrapli = {"auth_strict_key": True, "ssh_known_hosts_file": known_hosts}
    return {
        "netmiko": dict(netmiko),
        "scrapli": dict(scrapli),
        "napalm": dict(netmiko),
        "ncclient": {"hostkey_verify": True, "ssh_config": str(ssh_config_for(known_hosts))},
        "scrapli_netconf": dict(scrapli),
    }
