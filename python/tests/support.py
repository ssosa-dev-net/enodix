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

import json
import os
import re
import secrets
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, Dict, Optional

FIXTURE = Path(__file__).parent / "fixtures" / "workspace"
#: The fixture's workspace id: the scope the agent serves profiles for.
SCOPE = "python-bridge-test"
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
        assert self.process.stdout is not None
        while self.port is None or self.device_port is None or not self.agent:
            line = self.process.stdout.readline()
            if not line:
                self.close()
                raise RuntimeError(f"the test server stopped: {self.process.stderr.read()}")
            if line.startswith("credential agent: "):
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


class BridgeTest(unittest.TestCase):
    """A fixture copy and its server for each test; ``devices/`` reaches
    the device-mode server and uses the ``lab`` profile."""

    def setUp(self) -> None:
        required("NETTERM_CLI")
        self.workspace = Workspace()
        self.addCleanup(self.workspace.close)
        self.server = Server(self.workspace)
        self.addCleanup(self.server.close)
        self.workspace.write(
            "devices/_folder.yaml",
            f"credential_profile: lab\nplatform: cisco_ios\nport: {self.server.device_port}\n",
        )
