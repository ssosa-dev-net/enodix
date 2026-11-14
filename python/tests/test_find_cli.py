"""Finding the headless binary: NETTERM_CLI, else the default install
location, else PATH, and a clear message when it is nowhere."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import netterm
from netterm import _cli

from support import required

EXE = f"{_cli.CLI_NAME}.exe" if sys.platform == "win32" else _cli.CLI_NAME
#: The installer's folder name (the app's name, from the wheel).
APP = _cli.PRODUCT_NAME or "app"


class FindCli(unittest.TestCase):
    def setUp(self) -> None:
        self.cli = required("NETTERM_CLI")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.temp = Path(temp.name)

    def environment(self, **values: str) -> "mock._patch[dict]":
        """Only ``values`` (and what finding programs needs)."""
        kept = {name: os.environ[name] for name in ("SYSTEMROOT", "PATHEXT") if name in os.environ}
        return mock.patch.dict(os.environ, {**kept, **values}, clear=True)

    def test_netterm_cli_is_used_first(self) -> None:
        local = self.temp / "local"
        (local / APP).mkdir(parents=True)
        (local / APP / EXE).write_bytes(b"")
        with self.environment(NETTERM_CLI=self.cli, LOCALAPPDATA=str(local), PATH=""):
            self.assertEqual(netterm.find_cli(), Path(self.cli))

    def test_netterm_cli_naming_no_file_is_an_error(self) -> None:
        missing = self.temp / "nothing" / EXE
        with self.environment(NETTERM_CLI=str(missing), PATH=str(Path(self.cli).parent)):
            with self.assertRaises(netterm.CliNotFound) as raised:
                netterm.find_cli()
        self.assertIn("NETTERM_CLI is set to", str(raised.exception))
        self.assertIn(str(missing), str(raised.exception))

    @unittest.skipUnless(sys.platform == "win32", "the default location is the Windows installer's")
    def test_the_default_install_location_comes_next(self) -> None:
        local = self.temp / "local"
        installed = local / APP / EXE
        installed.parent.mkdir(parents=True)
        installed.write_bytes(b"")
        elsewhere = self.temp / "path"
        elsewhere.mkdir()
        (elsewhere / EXE).write_bytes(b"")
        with self.environment(LOCALAPPDATA=str(local), PATH=str(elsewhere)):
            self.assertEqual(netterm.default_location(), installed)
            self.assertEqual(netterm.find_cli(), installed)

    def test_then_path(self) -> None:
        elsewhere = self.temp / "path"
        elsewhere.mkdir()
        on_path = elsewhere / EXE
        on_path.write_bytes(b"")
        if sys.platform != "win32":
            on_path.chmod(0o755)
        with self.environment(LOCALAPPDATA=str(self.temp / "none"), PATH=str(elsewhere)):
            self.assertEqual(Path(netterm.find_cli()).resolve(), on_path.resolve())

    def test_nowhere_says_where_it_looked(self) -> None:
        with self.environment(LOCALAPPDATA=str(self.temp / "none"), PATH=str(self.temp)):
            with self.assertRaises(netterm.CliNotFound) as raised:
                netterm.inventory(self.temp)
        message = str(raised.exception)
        self.assertIn("NETTERM_CLI", message)
        self.assertIn(f"{_cli.CLI_NAME} on PATH", message)
        if sys.platform == "win32":
            self.assertIn(str(self.temp / "none" / APP / EXE), message)

    def test_the_package_has_the_binary_s_version(self) -> None:
        printed = subprocess.run(
            [self.cli, "--version"], capture_output=True, text=True, check=True
        ).stdout.split()
        self.assertEqual(printed[0], _cli.CLI_NAME)
        # The same version, as Python writes it (scripts/build-wheel.mjs).
        version = printed[1].split("+")[0]
        for stage, short in (("-alpha.", "a"), ("-beta.", "b"), ("-rc.", "rc")):
            version = version.replace(stage, short)
        self.assertEqual(netterm.__version__, version)


if __name__ == "__main__":
    unittest.main()
