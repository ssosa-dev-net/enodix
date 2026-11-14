"""Loopback tunnels to the hosts behind jump hosts, opened by the headless
binary (``netterm-cli tunnel``) and kept open while this process runs.

The binary connects each jump chain as the app would (the jump hosts'
stored logins, the host keys the app trusts) and listens on ``127.0.0.1``,
one port per host. A library connects there instead of to the host, and
checks the device's own host key against a known-hosts file written here
from the keys the app trusts (``127.0.0.1 <key>`` and ``[127.0.0.1]:<port>
<key>``, one file per host, as scrapli's paramiko transport ignores the
port). The tunnels end when :meth:`Tunnels.close` is called, when this
process exits, or when it is killed: the binary serves until its standard
input, held here, ends.
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
from typing import Any, Dict, List, Optional, Sequence

from ._cli import CLI_NAME, FORMAT, CliFailed, NettermError, find_cli

LOG = logging.getLogger("netterm")

#: Every tunnel process this one started and has not closed.
_OPEN: List["Tunnels"] = []
_OPEN_LOCK = threading.Lock()


class Tunnels:
    """A running ``netterm-cli tunnel``: ``hosts`` maps each host behind
    jump hosts (host id) to ``{"address", "port", "host_keys",
    "known_hosts"}`` when it is tunnelled, or to ``{"error"}`` saying why
    not. Use as a context manager, or call :meth:`close`; it is closed at
    exit anyway."""

    def __init__(self, args: Sequence[str]) -> None:
        cli = find_cli()
        try:
            self._process = subprocess.Popen(
                [str(cli), "tunnel", *args],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        except OSError as error:
            raise NettermError(f"cannot start {cli}: {error}") from error
        self.hosts: Dict[str, Dict[str, Any]] = {}
        self._folder: Optional[Path] = None
        assert self._process.stdout is not None and self._process.stderr is not None
        # Read from the start, so that the binary never waits on a full
        # stderr pipe; kept until the first line is in, logged after.
        self._early: Optional[List[str]] = []
        self._early_lock = threading.Lock()
        reader = threading.Thread(target=self._report, name="netterm tunnel stderr", daemon=True)
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
                raise NettermError(
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
                self._folder = Path(tempfile.mkdtemp(prefix="netterm-tunnels-"))
            address, port = entry["address"], entry["port"]
            known_hosts = self._folder / f"known_hosts_{len(self.hosts)}"
            lines = []
            for key in entry["host_keys"]:
                lines.append(f"{address} {key}\n")
                lines.append(f"[{address}]:{port} {key}\n")
            known_hosts.write_text("".join(lines), encoding="utf-8")
            self.hosts[host] = {
                "address": address,
                "port": port,
                "host_keys": list(entry["host_keys"]),
                "known_hosts": str(known_hosts),
            }

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


def host_key_options(tunnel: Dict[str, Any]) -> Dict[str, Dict[str, Any]]:
    """For a tunnelled host: the options that make each library check the
    device's host key against the keys the app trusts (by library:
    ``netmiko``, ``scrapli``, and ``napalm``'s ``optional_args``, which its
    Netmiko-based drivers pass on)."""
    netmiko = {
        "ssh_strict": True,
        "system_host_keys": False,
        "alt_host_keys": True,
        "alt_key_file": tunnel["known_hosts"],
    }
    return {
        "netmiko": dict(netmiko),
        "scrapli": {"auth_strict_key": True, "ssh_known_hosts_file": tunnel["known_hosts"]},
        "napalm": dict(netmiko),
    }
