"""Fill in from devices against devices that answer wrongly, slowly or not
at all (#119). NAPALM is stood in for by drivers that misbehave; the
inventory and the logins come from the real binary and the test
credential agent, as in ``test_enrich.py``."""

from __future__ import annotations

import json
import os
import socket
import time
from typing import Any, Callable, Dict, List
from unittest import mock

from enodix import enrich

from support import BridgeTest


def driver_of(behave: Callable[[Dict[str, Any]], Any], closing: Callable[[], None] = lambda: None) -> Any:
    """A NAPALM driver finder whose devices answer ``get_facts`` with what
    ``behave(arguments)`` returns (or raises)."""

    def find(name: str) -> Any:
        class Device:
            def __init__(self, **arguments: Any) -> None:
                self.arguments = dict(arguments, driver=name)

            def open(self) -> None:
                pass

            def get_facts(self) -> Any:
                return behave(self.arguments)

            def close(self) -> None:
                closing()

        return Device

    return find


class Misbehaving(BridgeTest):
    def job(self, hosts: List[str]) -> "mock._patch[dict]":
        self.targets = {
            "format": 1,
            "workspace": str(self.workspace.root),
            "hosts": hosts,
            "napalm_drivers": {host: "ios" for host in hosts},
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

    def hosts(self, count: int) -> List[str]:
        names = [f"devices/d{n:02}.yaml" for n in range(count)]
        for name in names:
            self.workspace.write(name, "address: 127.0.0.1\n")
        return names

    def collect(self, hosts: List[str], driver: Any) -> Dict[str, Dict[str, Any]]:
        lines: List[str] = []
        with self.job(hosts):
            entries = enrich.collect(self.targets, self.facts_file, driver, lines.append)
        self.lines = lines
        # What the app reads is what was returned, every host once.
        written = json.loads(self.facts_file.read_text(encoding="utf-8"))["hosts"]
        self.assertEqual(entries, written)
        self.assertEqual([entry["id"] for entry in entries], hosts)
        return {entry["id"]: entry for entry in entries}

    def test_wrong_answers_fail_their_host_or_keep_only_what_reads_as_text(self) -> None:
        [host] = self.hosts(1)

        def answering(answer: Any) -> Any:
            def behave(arguments: Dict[str, Any]) -> Any:
                if isinstance(answer, Exception):
                    raise answer
                return answer

            return driver_of(behave)

        # Not a mapping at all: the host fails, naming what went wrong.
        for answer in (None, ["not", "a", "mapping"], "text"):
            with self.subTest(answer=answer):
                error = self.collect([host], answering(answer))[host]["error"]
                self.assertTrue(error.startswith("NAPALM (ios) failed: AttributeError"), error)
        # Values of other types are kept as text; blank and missing ones are
        # left out (the app keeps what the host has for them).
        odd = {"vendor": None, "model": 7200, "serial_number": "  ", "os_version": "15.2\n"}
        self.assertEqual(
            self.collect([host], answering(odd))[host]["facts"], {"model": "7200", "os_version": "15.2"}
        )
        self.assertEqual(self.collect([host], answering({}))[host]["facts"], {})
        self.assertIn(f"{host}: no facts reported", self.lines)
        error = self.collect([host], answering(KeyError("serial_number")))[host]["error"]
        self.assertEqual(error, "NAPALM (ios) failed: KeyError: 'serial_number'")

    def test_a_close_that_fails_keeps_the_facts(self) -> None:
        hosts = self.hosts(1)

        def closing() -> None:
            raise OSError("socket closed by the device")

        facts = {"vendor": "Cisco", "model": "C8000V", "serial_number": "S1", "os_version": "17.9"}
        by_id = self.collect(hosts, driver_of(lambda a: facts, closing))
        self.assertEqual(by_id[hosts[0]]["facts"], facts)

    def test_slow_devices_are_asked_side_by_side_and_each_is_kept_as_it_answers(self) -> None:
        hosts = self.hosts(12)
        seen: List[int] = []

        def read_whole() -> str:
            # On Windows a reader can be refused for a moment while the file
            # is replaced (seen on a busy machine); what it reads once it is
            # let in must be whole. The app itself reads only after the
            # script has ended.
            for _ in range(100):
                try:
                    return self.facts_file.read_text(encoding="utf-8")
                except PermissionError:
                    time.sleep(0.02)
            return self.facts_file.read_text(encoding="utf-8")

        def behave(arguments: Dict[str, Any]) -> Any:
            time.sleep(0.4)
            # The facts file is whole whenever it is read meanwhile.
            seen.append(len(json.loads(read_whole())["hosts"]))
            return {"vendor": "Slow", "model": "S", "serial_number": "1", "os_version": "1"}

        started = time.monotonic()
        by_id = self.collect(hosts, driver_of(behave))
        took = time.monotonic() - started
        self.assertTrue(all("facts" in entry for entry in by_id.values()), by_id)
        # Ten at once: well under one after the other (12 x 0.4 s).
        self.assertLess(took, 12 * 0.4, took)
        self.assertEqual(len(seen), 12)

    def test_a_device_that_does_not_answer_fails_with_the_timeout_it_was_given(self) -> None:
        hosts = self.hosts(2)
        self.workspace.write(hosts[1], "address: 127.0.0.1\nconnect_timeout: 3\n")
        given: List[Any] = []

        def behave(arguments: Dict[str, Any]) -> Any:
            given.append(arguments["timeout"])
            raise socket.timeout("timed out")

        by_id = self.collect(hosts, driver_of(behave))
        # socket.timeout is TimeoutError from Python 3.10 on.
        name = type(socket.timeout()).__name__
        for host in hosts:
            self.assertEqual(by_id[host]["error"], f"NAPALM (ios) failed: {name}: timed out")
        # The host's own connect timeout, else the app's default (15 s, as
        # the inventory exports it).
        self.assertEqual(sorted(given), [3, 15])
