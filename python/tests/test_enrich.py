"""Fill in from devices (``python -m netterm.enrich``): the facts of the
job's hosts fetched with each host's own login and written to the facts
file. NAPALM is stood in for by a recording driver; the inventory and the
logins come from the real binary and the test credential agent."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List
from unittest import mock

from netterm import enrich

from support import BridgeTest


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
                "NETTERM_WORKSPACE": str(self.workspace.root),
                "NETTERM_JOB_TARGETS": str(path),
                "NETTERM_FACTS_FILE": str(self.facts_file),
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
        self.assertEqual(driver.closed, 2)
        self.assertTrue(any(line.startswith("devices/r-pass.yaml: vendor Cisco") for line in lines), lines)
        # No secret in what the script prints.
        self.assertFalse(any(lab["auth"]["password"] in line for line in lines))

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

    def test_run_by_hand_it_says_what_runs_it(self) -> None:
        environment = {k: v for k, v in os.environ.items() if k not in ("NETTERM_JOB_TARGETS", "NETTERM_FACTS_FILE")}
        done = subprocess.run(
            [sys.executable, "-m", "netterm.enrich"],
            env=environment,
            capture_output=True,
            text=True,
            cwd=Path(self.workspace.root),
        )
        self.assertEqual(done.returncode, 2)
        self.assertIn("Fill in from devices", done.stderr)
