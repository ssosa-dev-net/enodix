"""What the bridge's tests share: the built binary, a copy of the fixture
workspace, and the test SSH server with the app's credential agent.

``scripts/test-python.mjs`` (``npm run test:python``) builds the binary,
the example server and the wheel, installs the wheel into a virtual
environment and runs these tests with:

- ``NETTERM_CLI``: the built ``netterm-cli``;
- ``NETTERM_TEST_SSH_SERVER``: the built ``test_ssh_server`` example
  (``crates/core/examples``), started here with ``--credential-agent``: it
  serves test profiles from memory as the running app would, so the binary
  never reads a real keychain or vault.

Every test runs with its own ``NETTERM_DATA_DIR`` and a
``netterm-test-<hex>`` ``NETTERM_KEYCHAIN_PREFIX``.
"""

from __future__ import annotations

import faulthandler
import json
import os
import re
import secrets
import shutil
import socket
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path
from typing import Any, Dict, List, Optional

FIXTURE = Path(__file__).parent / "fixtures" / "workspace"
#: The fixture's workspace id: the scope the agent serves profiles for.
SCOPE = "python-bridge-test"
#: Seconds after which a test counts as hung.
HANG_AFTER = 180
#: The device-mode server's hostname (the example's `--device`).
DEVICE_HOSTNAME = "lab-r1"


def required(variable: str) -> str:
    value = os.environ.get(variable)
    if not value or not Path(value).is_file():
        raise unittest.SkipTest(
            f"{variable} does not name a built file: run these tests with npm run test:python"
        )
    return value


class Workspace:
    """A temporary copy of the fixture workspace, with a data folder and a
    keychain prefix of its own set in the environment while it lives."""

    def __init__(self) -> None:
        self._temp = tempfile.TemporaryDirectory(prefix="netterm-bridge-")
        temp = Path(self._temp.name)
        self.root = temp / "workspace"
        shutil.copytree(FIXTURE, self.root)
        self.data = temp / "data"
        self.data.mkdir()
        self._saved = {
            name: os.environ.get(name) for name in ("NETTERM_DATA_DIR", "NETTERM_KEYCHAIN_PREFIX")
        }
        os.environ["NETTERM_DATA_DIR"] = str(self.data)
        os.environ["NETTERM_KEYCHAIN_PREFIX"] = f"netterm-test-{secrets.token_hex(8)}"

    def write(self, relative: str, text: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8", newline="\n")

    def close(self) -> None:
        for name, value in self._saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        self._temp.cleanup()


class Server:
    """The example test SSH server (a plain one and a device-mode one) and
    the credential agent for ``workspace``'s data folder."""

    def __init__(self, workspace: Workspace) -> None:
        program = required("NETTERM_TEST_SSH_SERVER")
        self.process = subprocess.Popen(
            [
                program,
                "0",
                "--device",
                "0",
                "--credential-agent",
                str(workspace.data),
                SCOPE,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        self.port: Optional[int] = None
        self.device_port: Optional[int] = None
        self.agent: Dict[str, Any] = {}
        #: The device-mode server's host key (``<type> <base64>``).
        self.device_key: Optional[str] = None
        assert self.process.stdout is not None
        while self.port is None or self.device_port is None or not self.agent or not self.device_key:
            line = self.process.stdout.readline()
            if not line:
                self.close()
                raise RuntimeError(f"the test server stopped: {self.process.stderr.read()}")
            if line.startswith("device host key: "):
                self.device_key = key_text(line.removeprefix("device host key: "))
            elif line.startswith("credential agent: "):
                self.agent = json.loads(line.removeprefix("credential agent: "))
            elif match := re.match(r"test SSH server listening on 127\.0\.0\.1:(\d+)", line):
                self.port = int(match[1])
            elif match := re.match(r"device \S+ \(device-mode shell\) listening on 127\.0\.0\.1:(\d+)", line):
                self.device_port = int(match[1])

    def profile(self, name: str) -> Dict[str, Any]:
        return next(p for p in self.agent["profiles"] if p["name"] == name)

    def close(self) -> None:
        self.process.kill()
        self.process.wait()
        for stream in (self.process.stdout, self.process.stderr):
            if stream:
                stream.close()


def key_text(openssh: str) -> str:
    """``<type> <base64>`` of an OpenSSH public key line (no comment)."""
    return " ".join(openssh.split()[:2])


class JumpServer:
    """Another example server: a plain SSH server, as a jump host (it
    forwards connections), with the same login as the device's."""

    def __init__(self) -> None:
        self.process = subprocess.Popen(
            [required("NETTERM_TEST_SSH_SERVER"), "0"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
        )
        self.port: Optional[int] = None
        self.key: Optional[str] = None
        assert self.process.stdout is not None
        while self.port is None or self.key is None:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("the jump server stopped")
            if line.startswith("host key: "):
                self.key = key_text(line.removeprefix("host key: "))
            elif match := re.match(r"test SSH server listening on 127\.0\.0\.1:(\d+)", line):
                self.port = int(match[1])

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait()
        if self.process.stdout:
            self.process.stdout.close()


class Relay:
    """A TCP service on its own port of ``127.0.0.1`` that passes every
    connection on to ``target`` (a port of ``127.0.0.1``): through it, a
    second port of the device shows the same SSH service, host key
    included."""

    def __init__(self, target: int) -> None:
        self.target = target
        self._listener = socket.create_server(("127.0.0.1", 0))
        self.port: int = self._listener.getsockname()[1]
        self._open: List[socket.socket] = []
        self._guard = threading.Lock()
        threading.Thread(target=self._accept, name="relay", daemon=True).start()

    def _accept(self) -> None:
        while True:
            try:
                client, _ = self._listener.accept()
            except OSError:
                return
            try:
                upstream = socket.create_connection(("127.0.0.1", self.target), timeout=10)
                upstream.settimeout(None)
            except OSError:
                client.close()
                continue
            with self._guard:
                self._open += [client, upstream]
            for source, sink in ((client, upstream), (upstream, client)):
                threading.Thread(target=self._pump, args=(source, sink), daemon=True).start()

    @staticmethod
    def _pump(source: socket.socket, sink: socket.socket) -> None:
        try:
            while data := source.recv(65536):
                sink.sendall(data)
        except OSError:
            pass
        for end in (source, sink):
            try:
                end.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    def close(self) -> None:
        self._listener.close()
        with self._guard:
            for end in self._open:
                end.close()


class BridgeTest(unittest.TestCase):
    """A fixture copy and its server for each test; ``devices/`` reaches
    the device-mode server and uses the ``lab`` profile."""

    def setUp(self) -> None:
        required("NETTERM_CLI")
        # A test that hangs ends the run with every thread's traceback.
        faulthandler.dump_traceback_later(HANG_AFTER, exit=True)
        self.addCleanup(faulthandler.cancel_dump_traceback_later)
        self.workspace = Workspace()
        self.addCleanup(self.workspace.close)
        self.server = Server(self.workspace)
        self.addCleanup(self.server.close)
        self.workspace.write(
            "devices/_folder.yaml",
            f"credential_profile: lab\nplatform: cisco_ios\nport: {self.server.device_port}\n",
        )
