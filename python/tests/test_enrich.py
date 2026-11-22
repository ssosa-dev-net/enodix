"""Fill in from devices (``python -m enodix.enrich``): the facts of the
job's hosts fetched with each host's own login and written to the facts
file. NAPALM is stood in for by a recording driver; the inventory and the
logins come from the real binary and the test credential agent."""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List
from unittest import mock

from enodix import enrich

from support import BridgeTest, JumpServer


class Recording:
    """A NAPALM driver class whose devices report given facts, or fail."""

    def __init__(self, facts: Dict[str, Dict[str, Any]]) -> None:
        self.facts = facts
        self.opened: List[Dict[str, Any]] = []
        self.closed = 0

    def __call__(self, name: str) -> Any:
        recording = self

        class Device:
            def __init__(self, **arguments: Any) -> None:
                self.arguments = dict(arguments, driver=name)

            def open(self) -> None:
                recording.opened.append(self.arguments)
                reported = recording.facts[self.arguments["hostname"] + str(self.arguments["optional_args"].get("port"))]
                if isinstance(reported, Exception):
                    raise reported

            def get_facts(self) -> Dict[str, Any]:
                return recording.facts[self.arguments["hostname"] + str(self.arguments["optional_args"].get("port"))]

            def close(self) -> None:
                recording.closed += 1

        return Device


class FillIn(BridgeTest):
    def setUp(self) -> None:
        super().setUp()
        self.workspace.write(
            "devices/sw-eos.yaml",
            "address: 127.0.0.2\nplatform: eos\nport: 2201\n",
        )

    def job(self, hosts: Dict[str, str]) -> "mock._patch[dict]":
        self.targets = {
            "format": 1,
            "workspace": str(self.workspace.root),
            "hosts": list(hosts),
            "napalm_drivers": {host: driver for host, driver in hosts.items() if driver},
        }
        path = self.workspace.data / "targets.json"
        path.write_text(json.dumps(self.targets), encoding="utf-8")
        self.facts_file = self.workspace.data / "facts.json"
        return mock.patch.dict(
            os.environ,
            {
                "ENODIX_WORKSPACE": str(self.workspace.root),
                "ENODIX_JOB_TARGETS": str(path),
                "ENODIX_FACTS_FILE": str(self.facts_file),
            },
        )

    def written(self) -> List[Dict[str, Any]]:
        file = json.loads(self.facts_file.read_text(encoding="utf-8"))
        self.assertEqual(file["format"], 1)
        return file["hosts"]

    def test_each_host_is_asked_with_its_own_login_and_driver(self) -> None:
        port = self.server.device_port
        driver = Recording(
            {
                f"127.0.0.1{port}": {
                    "vendor": "Cisco",
                    "model": " CSR1000V ",
                    "serial_number": "9ABCDEF",
                    "os_version": "17.3.1a",
                    "hostname": "lab-r1",
                    "uptime": 12.5,
                },
                "127.0.0.22201": RuntimeError("Authentication failed.\nmore text"),
            }
        )
        lines: List[str] = []
        hosts = {
            "devices/r-pass.yaml": "ios",
            "devices/r-key.yaml": "ios",
            "devices/sw-eos.yaml": "eos",
            "devices/r-missing.yaml": "ios",
        }
        # The app trusts their keys (as after a first session to each).
        (self.workspace.data / "known_hosts").write_text(
            f"[127.0.0.1]:{port} {self.server.device_key}\n[127.0.0.2]:2201 {self.server.device_key}\n",
            encoding="utf-8",
        )
        # r-key's facts are those of r-pass: both reach the device server.
        with self.job(hosts):
            entries = enrich.collect(self.targets, self.facts_file, driver, lines.append)
        self.assertEqual(entries, self.written())
        by_id = {entry["id"]: entry for entry in entries}
        self.assertEqual(list(by_id), list(hosts))
        expected = {
            "vendor": "Cisco",
            "model": "CSR1000V",
            "serial_number": "9ABCDEF",
            "os_version": "17.3.1a",
        }
        self.assertEqual(by_id["devices/r-pass.yaml"], {"id": "devices/r-pass.yaml", "facts": expected})
        self.assertEqual(by_id["devices/r-key.yaml"]["facts"], expected)
        self.assertEqual(
            by_id["devices/sw-eos.yaml"]["error"], "NAPALM (eos) failed: RuntimeError: Authentication failed."
        )
        self.assertIn("nowhere", by_id["devices/r-missing.yaml"]["error"])

        opened = {(a["hostname"], a["optional_args"].get("key_file") is not None): a for a in driver.opened}
        lab = self.server.profile("lab")
        password = opened[("127.0.0.1", False)]
        self.assertEqual(password["driver"], "ios")
        self.assertEqual(password["username"], lab["username"])
        self.assertEqual(password["password"], lab["auth"]["password"])
        self.assertEqual(password["optional_args"]["port"], port)
        self.assertEqual(password["optional_args"]["secret"], lab["enable_secret"])
        key = opened[("127.0.0.1", True)]
        self.assertTrue(key["optional_args"]["use_keys"])
        self.assertEqual(key["password"], "")
        eos = opened[("127.0.0.2", False)]
        self.assertEqual(eos["optional_args"]["transport"], "ssh")
        self.assertEqual(eos["optional_args"]["port"], 2201)
        # Reached directly, the device's key is checked against the app's.
        for arguments in (password, key, eos):
            optional = arguments["optional_args"]
            self.assertTrue(optional["ssh_strict"])
            self.assertFalse(optional["system_host_keys"])
            self.assertIn(self.server.device_key, Path(optional["alt_key_file"]).read_text(encoding="utf-8"))
        self.assertEqual(driver.closed, 2)
        self.assertTrue(any(line.startswith("devices/r-pass.yaml: vendor Cisco") for line in lines), lines)
        # No secret in what the script prints.
        self.assertFalse(any(lab["auth"]["password"] in line for line in lines))

    def test_hosts_behind_a_jump_host_are_asked_through_its_tunnel(self) -> None:
        bastion = JumpServer()
        self.addCleanup(bastion.close)
        port = self.server.device_port
        self.workspace.write(
            "jumps/bastion.yaml", f"address: 127.0.0.1\nport: {bastion.port}\ncredential_profile: lab\n"
        )
        self.workspace.write(
            "behind/r1.yaml",
            f"address: 127.0.0.1\nport: {port}\nplatform: ios\ncredential_profile: lab\n"
            "jump_host: jumps/bastion.yaml\n",
        )
        self.workspace.write(
            "behind/new.yaml",
            "address: 192.0.2.8\nplatform: ios\ncredential_profile: lab\njump_host: jumps/bastion.yaml\n",
        )
        (self.workspace.data / "known_hosts").write_text(
            f"[127.0.0.1]:{bastion.port} {bastion.key}\n[127.0.0.1]:{port} {self.server.device_key}\n",
            encoding="utf-8",
        )
        reached: List[Dict[str, Any]] = []

        def driver(name: str) -> Any:
            class Device:
                def __init__(self, **arguments: Any) -> None:
                    self.arguments = arguments

                def open(self) -> None:
                    # Through the tunnel, the device's SSH banner.
                    optional = self.arguments["optional_args"]
                    with socket.create_connection((self.arguments["hostname"], optional["port"]), 10) as s:
                        self.arguments["banner"] = s.recv(64).decode("ascii", "replace")
                    self.arguments["known"] = Path(optional["alt_key_file"]).read_text(encoding="utf-8")
                    reached.append(self.arguments)

                def get_facts(self) -> Dict[str, Any]:
                    return {"vendor": "Cisco", "model": "CSR1000V", "serial_number": "X", "os_version": "17"}

                def close(self) -> None:
                    pass

            return Device

        with self.job({"behind/r1.yaml": "ios", "behind/new.yaml": "ios"}):
            entries = enrich.collect(self.targets, self.facts_file, driver, lambda line: None)
        by_id = {entry["id"]: entry for entry in entries}
        self.assertIn("facts", by_id["behind/r1.yaml"], by_id)
        self.assertIn("connect to it once in the app", by_id["behind/new.yaml"]["error"])
        [arguments] = reached
        self.assertEqual(arguments["hostname"], "127.0.0.1")
        optional = arguments["optional_args"]
        self.assertNotEqual(optional["port"], port)
        self.assertTrue(arguments["banner"].startswith("SSH-2.0-"))
        # NAPALM's ios driver (Netmiko) checks the device's key.
        self.assertTrue(optional["ssh_strict"])
        self.assertIn(self.server.device_key, arguments["known"])
        # The tunnels end with the collection.
        self.assertFalse(Path(optional["alt_key_file"]).exists())

    def test_a_direct_host_whose_key_the_app_has_not_trusted_is_not_logged_in_to(self) -> None:
        # No known_hosts at all: whoever answers at the address would get
        # the login, so none is fetched and the driver never opens.
        driver = Recording({})
        with self.job({"devices/r-pass.yaml": "ios"}):
            with mock.patch.object(enrich, "credentials", side_effect=AssertionError("login fetched")):
                entries = enrich.collect(self.targets, self.facts_file, driver, lambda line: None)
        [entry] = entries
        self.assertIn("has not trusted its host key", entry["error"])
        self.assertIn("connect to it once in the app", entry["error"])
        self.assertEqual(driver.opened, [])

    def test_without_napalm_every_host_fails_saying_so(self) -> None:
        with self.job({"devices/r-pass.yaml": "ios", "devices/r-key.yaml": "ios"}):
            with mock.patch.dict(sys.modules, {"napalm": None}):
                entries = enrich.collect(self.targets, self.facts_file, None, lambda line: None)
        self.assertEqual(len(entries), 2)
        for entry in entries:
            self.assertIn("NAPALM is not installed", entry["error"])

    def test_the_driver_arguments_follow_the_driver(self) -> None:
        host = {"address": "10.0.0.1", "settings": {"port": 22, "connect_timeout": 15}}
        agent = {"username": "u", "auth": {"kind": "agent"}, "enable_secret": None}
        junos = enrich.driver_arguments(host, agent, "junos")
        self.assertEqual(junos["timeout"], 15)
        self.assertEqual(junos["optional_args"], {"port": 22, "use_keys": True, "allow_agent": True})
        ios = enrich.driver_arguments(host, agent, "ios")
        self.assertEqual(ios["optional_args"]["port"], 22)

    def test_through_a_tunnel_the_drivers_that_use_netmiko_check_the_device_key(self) -> None:
        host = {"address": "10.0.0.1", "settings": {"port": 22, "connect_timeout": 15}}
        login = {"username": "u", "auth": {"kind": "password", "password": "p"}, "enable_secret": None}
        tunnel = {"address": "127.0.0.1", "port": 50022, "host_keys": [], "known_hosts": "keys"}
        for driver in ("ios", "nxos_ssh", "eos"):
            with self.subTest(driver=driver):
                arguments = enrich.driver_arguments(host, login, driver, tunnel)
                self.assertEqual(arguments["hostname"], "127.0.0.1")
                optional = arguments["optional_args"]
                self.assertEqual(optional["port"], 50022)
                self.assertTrue(optional["ssh_strict"])
                self.assertEqual(optional["alt_key_file"], "keys")
        # Junos logs in without Netmiko, which is what takes the file.
        junos = enrich.driver_arguments(host, login, "junos", tunnel)["optional_args"]
        self.assertNotIn("ssh_strict", junos)
        self.assertEqual(junos["port"], 50022)

    def test_run_by_hand_it_says_what_runs_it(self) -> None:
        environment = {k: v for k, v in os.environ.items() if k not in ("ENODIX_JOB_TARGETS", "ENODIX_FACTS_FILE")}
        done = subprocess.run(
            [sys.executable, "-m", "enodix.enrich"],
            env=environment,
            capture_output=True,
            text=True,
            cwd=Path(self.workspace.root),
        )
        self.assertEqual(done.returncode, 2)
        self.assertIn("Fill in from devices", done.stderr)
