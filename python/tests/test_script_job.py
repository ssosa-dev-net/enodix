"""Run as a script job: the environment the app sets (``NETTERM_WORKSPACE``
and ``NETTERM_JOB_TARGETS``, a file listing the job's hosts) limits the
inventory and the logins to the job's hosts, and names the workspace."""

from __future__ import annotations

import json
import os
from typing import List
from unittest import mock

import netterm
from netterm.nornir import NettermInventory

from support import BridgeTest


class ScriptJob(BridgeTest):
    def job(self, hosts: List[str]) -> "mock._patch[dict]":
        targets = self.workspace.data / "targets.json"
        targets.write_text(
            json.dumps({"format": 1, "workspace": str(self.workspace.root), "hosts": hosts}),
            encoding="utf-8",
        )
        return mock.patch.dict(
            os.environ,
            {"NETTERM_WORKSPACE": str(self.workspace.root), "NETTERM_JOB_TARGETS": str(targets)},
        )

    def test_the_inventory_holds_only_the_jobs_hosts(self) -> None:
        with self.job(["devices/r-pass.yaml", "Madrid/core-r1.yaml"]):
            self.assertEqual(netterm.job_workspace(), str(self.workspace.root))
            # No workspace given: the job's.
            with self.assertLogs("netterm", "WARNING"):  # core-r1 is behind a jump host
                inventory = NettermInventory(credentials=False).load()
            self.assertEqual(set(inventory.hosts), {"devices/r-pass.yaml", "Madrid/core-r1.yaml"})
            self.assertEqual(set(inventory.groups), {"devices", "Madrid"})
            limited = NettermInventory(folder="devices").load()
            self.assertEqual(set(limited.hosts), {"devices/r-pass.yaml"})
            lab = self.server.profile("lab")
            self.assertEqual(limited.hosts["devices/r-pass.yaml"].username, lab["username"])

    def test_other_hosts_logins_are_refused(self) -> None:
        with self.job(["devices/r-pass.yaml"]):
            with self.assertRaises(netterm.CliFailed) as refused:
                netterm.credentials(None, "devices/r-key.yaml")
            self.assertIn("not among the hosts of the job", str(refused.exception))
            with self.assertRaises(netterm.CliFailed):
                netterm.inventory(hosts=["devices/r-key.yaml"])

    def test_without_a_job_a_workspace_must_be_given(self) -> None:
        with mock.patch.dict(os.environ):
            os.environ.pop("NETTERM_WORKSPACE", None)
            self.assertIsNone(netterm.job_workspace())
            with self.assertRaises(netterm.NettermError):
                netterm.inventory()
