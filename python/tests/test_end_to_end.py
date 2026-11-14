"""One Nornir task (nornir_netmiko) logging in to the test SSH server with
the credentials of the hosts' profiles, handed over by the binary from the
credential agent: nothing typed, nothing in a file or the environment."""

from __future__ import annotations

import unittest

from nornir import InitNornir
from nornir_netmiko.tasks import netmiko_send_command

from support import DEVICE_HOSTNAME, BridgeTest


class NornirTask(BridgeTest):
    def test_a_task_logs_in_with_each_profile_s_credentials(self) -> None:
        nr = InitNornir(
            runner={"plugin": "threaded", "options": {"num_workers": 2}},
            inventory={
                "plugin": "NettermInventory",
                "options": {
                    "workspace": str(self.workspace.root),
                    "hosts": ["devices/r-pass.yaml", "devices/r-key.yaml"],
                },
            },
            logging={"enabled": False},
        )
        self.addCleanup(nr.close_connections)
        results = nr.run(task=netmiko_send_command, command_string="show hostname")
        self.assertEqual(set(results), {"devices/r-pass.yaml", "devices/r-key.yaml"})
        for host, result in results.items():
            with self.subTest(host=host):
                self.assertFalse(result.failed, result[0].exception)
                self.assertIn(DEVICE_HOSTNAME, result[0].result)


if __name__ == "__main__":
    unittest.main()
