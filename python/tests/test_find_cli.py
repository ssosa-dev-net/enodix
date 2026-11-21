"""Finding the headless binary: NETTERM_CLI, else the default install
location, else PATH, and a clear message when it is nowhere."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
import warnings
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
        self.assertEqual(_cli.python_version(printed[1]), netterm.__version__)

    def test_app_versions_are_written_as_python_writes_them(self) -> None:
        for app, python in (
            ("0.4.1", "0.4.1"),
            ("1.2.3-beta.1", "1.2.3b1"),
            ("1.2.3-alpha.2", "1.2.3a2"),
            ("1.2.3-rc.10", "1.2.3rc10"),
            ("1.2.3+build.5", "1.2.3"),
        ):
            self.assertEqual(_cli.python_version(app), python)

    def compared(self) -> "list[warnings.WarningMessage]":
        """The version warnings of a first run of the binary, through
        :func:`netterm.inventory` (which then fails: no workspace there)."""
        with self.environment(NETTERM_CLI=self.cli), mock.patch.object(_cli, "_COMPARED", set()):
            with warnings.catch_warnings(record=True) as seen:
                warnings.simplefilter("always")
                for _ in range(2):
                    with self.assertRaises(netterm.CliFailed):
                        netterm.inventory(self.temp / "nothing")
        return [warning for warning in seen if issubclass(warning.category, netterm.VersionWarning)]

    def test_a_binary_of_another_version_is_warned_about_once(self) -> None:
        if _cli.PACKAGE_VERSION is None:
            self.skipTest("the source tree has no version to compare")
        with mock.patch.object(_cli, "PACKAGE_VERSION", "0.0.1"):
            warned = self.compared()
        self.assertEqual(len(warned), 1, warned)
        message = str(warned[0].message)
        self.assertIn("0.0.1", message)
        self.assertIn(netterm.__version__, message)
        self.assertIn(self.cli, message)
        self.assertIn("install the package that ships with the app", message)

    def test_the_binary_of_the_package_s_version_is_not_warned_about(self) -> None:
        if _cli.PACKAGE_VERSION is None:
            self.skipTest("the source tree has no version to compare")
        self.assertEqual(self.compared(), [])

    def test_values_that_start_with_a_dash_go_after_a_double_dash(self) -> None:
        self.assertEqual(_cli.values("C:/ws", "netbox-token"), ["C:/ws", "netbox-token"])
        self.assertEqual(_cli.values("C:/ws", "-ro"), ["--", "C:/ws", "-ro"])
        self.assertEqual(_cli.option("--folder", "Madrid"), ["--folder", "Madrid"])
        self.assertEqual(_cli.option("--folder", "-old"), ["--folder=-old"])


if __name__ == "__main__":
    unittest.main()
