"""The Nornir inventory plugin against the built binary and the fixture
workspace: hosts, groups (folders), data (tags, custom fields), limits,
logins from each kind of profile, jump hosts, and named secrets."""

from __future__ import annotations

import unittest
from unittest import mock

from nornir import InitNornir
from nornir.core.filter import F
from nornir.core.inventory import Inventory

import netterm
from netterm.nornir import NettermInventory

from support import SCOPE, BridgeTest

ALL_HOSTS = {
    "Madrid/Lab/lab-sw1.yaml",
    "Madrid/core-r1.yaml",
    "bastion.yaml",
    "devices/r-agent.yaml",
    "devices/r-key.yaml",
    "devices/r-missing.yaml",
    "devices/r-pass.yaml",
}


class Inventories(BridgeTest):
    def load(self, **options: object) -> Inventory:
        return NettermInventory(str(self.workspace.root), **options).load()  # type: ignore[arg-type]

    def test_hosts_groups_and_data(self) -> None:
        with self.assertLogs("netterm", "WARNING"):
            inventory = self.load(credentials=False)
        self.assertEqual(set(inventory.hosts), ALL_HOSTS)
        self.assertEqual(set(inventory.groups), {"Madrid", "Madrid/Lab", "devices"})

        lab = inventory.groups["Madrid/Lab"]
        self.assertEqual([group.name for group in lab.groups], ["Madrid"])
        self.assertEqual(lab.data, {"tags": ["lab"], "description": "Madrid lab"})
        self.assertEqual(inventory.groups["Madrid"].data["description"], "Madrid DC")

        core = inventory.hosts["Madrid/core-r1.yaml"]
        self.assertEqual(core.hostname, "10.1.0.1")
        self.assertEqual(core.port, 2222)
        self.assertEqual(core.platform, "arista_eos")
        self.assertEqual([group.name for group in core.groups], ["Madrid"])
        self.assertIsNone(core.username)
        self.assertEqual(sorted(core["tags"]), ["acme", "core", "edge", "madrid"])
        self.assertEqual(core["owner"], "noc")
        self.assertEqual(core["circuit"], "CID-1")
        self.assertEqual(core["site"], "MAD")
        self.assertEqual(core["role"], "router")
        # A built-in field wins over a custom field of the same name.
        self.assertEqual(core["vendor"], "arista")
        self.assertEqual(core["netterm"]["custom"]["vendor"], "not-the-vendor")
        # A secret-named custom field never reaches the inventory.
        self.assertNotIn("snmp_community", core.data)
        self.assertNotIn("snmp_community", core["netterm"]["custom"])
        self.assertEqual(core["netterm"]["withheld"], ["custom.snmp_community"])
        self.assertEqual(core["netterm"]["name"], "CORE-R1")
        self.assertEqual(core["netterm"]["folders"], ["Madrid"])
        self.assertEqual(core["netterm"]["settings"]["environment"], "production")

        bastion = inventory.hosts["bastion.yaml"]
        self.assertEqual(list(bastion.groups), [])
        self.assertEqual(bastion.port, 22)
        self.assertIsNone(bastion.platform)

        # Group data is inherited where the host has none.
        self.assertEqual(inventory.hosts["Madrid/Lab/lab-sw1.yaml"]["description"], "Madrid lab")

    def test_nornir_filters_on_tags_and_custom_fields(self) -> None:
        # By the plugin's registered name: the wheel's entry point.
        with self.assertLogs("netterm", "WARNING"):
            nr = InitNornir(
                inventory={
                    "plugin": "NettermInventory",
                    "options": {"workspace": str(self.workspace.root), "credentials": False},
                },
                logging={"enabled": False},
            )
        self.assertEqual(set(nr.filter(F(tags__contains="edge")).inventory.hosts), {"Madrid/core-r1.yaml"})
        self.assertEqual(set(nr.filter(owner="noc").inventory.hosts), {"Madrid/core-r1.yaml"})
        self.assertEqual(
            set(nr.filter(F(groups__contains="Madrid")).inventory.hosts),
            {"Madrid/core-r1.yaml"},
        )
        self.assertEqual(
            set(nr.filter(F(has_parent_group="Madrid")).inventory.hosts),
            {"Madrid/core-r1.yaml", "Madrid/Lab/lab-sw1.yaml"},
        )

    def test_limited_to_a_folder_a_filter_or_hosts(self) -> None:
        with self.assertLogs("netterm", "WARNING"):
            madrid = self.load(folder="Madrid", credentials=False)
        self.assertEqual(set(madrid.hosts), {"Madrid/core-r1.yaml", "Madrid/Lab/lab-sw1.yaml"})
        self.assertEqual(set(madrid.groups), {"Madrid", "Madrid/Lab"})

        with self.assertLogs("netterm", "WARNING"):
            edge = self.load(filter="edge", credentials=False)
        self.assertEqual(set(edge.hosts), {"Madrid/core-r1.yaml"})

        chosen = self.load(hosts=["bastion.yaml", "devices/r-pass.yaml"], credentials=False)
        self.assertEqual(list(chosen.hosts), ["bastion.yaml", "devices/r-pass.yaml"])
        self.assertEqual(set(chosen.groups), {"devices"})

        self.assertEqual(list(self.load(hosts="bastion.yaml").hosts), ["bastion.yaml"])

    def test_unknown_limits_and_workspaces_are_errors(self) -> None:
        with self.assertRaises(netterm.CliFailed) as raised:
            self.load(folder="Lisbon")
        self.assertEqual(raised.exception.status, 1)
        self.assertIn("there is no folder", str(raised.exception))
        self.assertIn("Lisbon", str(raised.exception))

        with self.assertRaises(netterm.CliFailed) as raised:
            self.load(filter="Nothing")
        self.assertIn("saved filters: Edge", str(raised.exception))

        with self.assertRaises(netterm.CliFailed) as raised:
            self.load(hosts=["nope.yaml"])
        self.assertIn("nope.yaml", str(raised.exception))

        with self.assertRaises(netterm.NettermError):
            NettermInventory(str(self.workspace.root / "missing")).load()

        with self.assertRaises(ValueError):
            netterm.inventory(self.workspace.root, folder="Madrid", filter="Edge")

    def test_hosts_behind_jump_hosts_are_marked(self) -> None:
        with self.assertLogs("netterm", "WARNING") as logged:
            inventory = self.load(credentials=False)
        core = inventory.hosts["Madrid/core-r1.yaml"]["netterm"]
        self.assertEqual(core["jump_hosts"], ["bastion.yaml"])
        self.assertTrue(core["behind_jump_host"])
        lab = inventory.hosts["Madrid/Lab/lab-sw1.yaml"]["netterm"]
        self.assertEqual(lab["jump_hosts"], [])
        self.assertIn("gone.yaml", lab["jump_chain_error"])
        self.assertTrue(lab["behind_jump_host"])
        for direct in ("bastion.yaml", "devices/r-pass.yaml"):
            self.assertFalse(inventory.hosts[direct]["netterm"]["behind_jump_host"])
            self.assertIsNone(inventory.hosts[direct]["netterm"]["tunnel_error"])
        # Without credentials no tunnel is opened (test_tunnels.py has them).
        self.assertIsNone(core["tunnel"])
        self.assertIn("tunnels off", core["tunnel_error"])
        warning = "\n".join(logged.output)
        self.assertIn("could not be tunnelled", warning)
        self.assertIn("Madrid/core-r1.yaml", warning)
        self.assertIn("Madrid/Lab/lab-sw1.yaml", warning)
        self.assertNotIn("bastion.yaml,", warning)


class Logins(BridgeTest):
    def load(self, *hosts: str) -> Inventory:
        return NettermInventory(str(self.workspace.root), hosts=list(hosts)).load()

    def test_a_password_profile_hands_over_username_and_password(self) -> None:
        lab = self.server.profile("lab")
        host = self.load("devices/r-pass.yaml").hosts["devices/r-pass.yaml"]
        self.assertEqual(host.username, lab["username"])
        self.assertEqual(host.password, lab["auth"]["password"])
        self.assertEqual(host["netterm"]["login"], "password")
        netmiko = host.get_connection_parameters("netmiko")
        self.assertEqual(netmiko.username, lab["username"])
        self.assertEqual(netmiko.password, lab["auth"]["password"])
        self.assertEqual(netmiko.extras, {"secret": lab["enable_secret"]})
        scrapli = host.get_connection_parameters("scrapli")
        self.assertEqual(scrapli.extras, {"auth_secondary": lab["enable_secret"]})
        # Secrets stay out of the data, which scripts print.
        self.assertNotIn(lab["auth"]["password"], repr(host.data))

    def test_a_key_profile_hands_over_the_key_file_and_passphrase(self) -> None:
        keys = self.server.profile("keys")
        host = self.load("devices/r-key.yaml").hosts["devices/r-key.yaml"]
        self.assertEqual(host.username, keys["username"])
        self.assertIsNone(host.password)
        self.assertEqual(host["netterm"]["login"], "private_key")
        key_file = keys["auth"]["key_file"]
        passphrase = keys["auth"]["passphrase"]
        self.assertEqual(
            host.get_connection_parameters("netmiko").extras,
            {"use_keys": True, "key_file": key_file, "passphrase": passphrase},
        )
        self.assertEqual(
            host.get_connection_parameters("scrapli").extras,
            {"auth_private_key": key_file, "auth_private_key_passphrase": passphrase},
        )
        self.assertEqual(
            host.get_connection_parameters("napalm").extras,
            {"optional_args": {"use_keys": True, "key_file": key_file, "passphrase": passphrase}},
        )

    def test_an_agent_profile_hands_over_nothing_secret(self) -> None:
        host = self.load("devices/r-agent.yaml").hosts["devices/r-agent.yaml"]
        self.assertEqual(host.username, self.server.profile("agent")["username"])
        self.assertIsNone(host.password)
        self.assertEqual(host["netterm"]["login"], "agent")
        self.assertEqual(
            host.get_connection_parameters("netmiko").extras,
            {"use_keys": True, "allow_agent": True},
        )

    def test_a_host_without_a_profile_has_no_login(self) -> None:
        host = self.load("bastion.yaml").hosts["bastion.yaml"]
        self.assertIsNone(host.username)
        self.assertIsNone(host.password)
        self.assertIsNone(host["netterm"]["login"])

    def test_a_profile_not_set_up_stops_the_load_and_says_which(self) -> None:
        with self.assertRaises(netterm.NettermError) as raised:
            self.load("devices/r-pass.yaml", "devices/r-missing.yaml")
        message = str(raised.exception)
        self.assertIn("devices/r-missing.yaml", message)
        self.assertIn("nowhere", message)
        self.assertIn("credentials=False", message)

    def test_one_question_per_profile(self) -> None:
        self.workspace.write("devices/r-pass2.yaml", "address: 127.0.0.2\n")
        with mock.patch("netterm.nornir.credentials", wraps=netterm.credentials) as asked:
            inventory = self.load("devices/r-pass.yaml", "devices/r-pass2.yaml", "devices/r-key.yaml")
        self.assertEqual(asked.call_count, 2)
        first, second, _ = inventory.hosts.values()
        self.assertEqual(first.password, self.server.profile("lab")["auth"]["password"])
        self.assertEqual(second.password, first.password)

    def test_the_agent_serves_this_workspace_s_scope(self) -> None:
        self.assertEqual(self.server.agent["scope"], SCOPE)


class Secrets(BridgeTest):
    def test_a_named_secret_is_read_by_name(self) -> None:
        expected = self.server.agent["secrets"]["netbox-token"]
        self.assertEqual(netterm.secret(self.workspace.root, "netbox-token"), expected)
        self.assertEqual(netterm.secret(str(self.workspace.root), "NETBOX-TOKEN"), expected)

    def test_an_unknown_secret_is_an_error(self) -> None:
        with self.assertRaises(netterm.CliFailed) as raised:
            netterm.secret(self.workspace.root, "nothing")
        self.assertIn("there is no secret called nothing", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
