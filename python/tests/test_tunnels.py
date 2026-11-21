"""Hosts behind jump hosts: the Nornir plugin opens ``netterm-cli tunnel``
and Netmiko logs in through it to the device-mode test server, reached only
through test jump hosts (other instances of the example server) that log in
with the stored ``lab`` password profile. The app's known hosts (in the
test's data folder) trust the jump hosts and the device, as if the app had
connected to them once."""

from __future__ import annotations

import json
import logging
import socket
import subprocess
import sys
import threading
import time
import unittest
from pathlib import Path
from typing import List

import paramiko
from nornir import InitNornir
from nornir.core import Nornir
from nornir_netmiko.tasks import netmiko_send_command

import netterm
from netterm._tunnel import _close_all as close_all_tunnels
from netterm.nornir import NettermInventory

from support import DEVICE_HOSTNAME, BridgeTest, JumpServer, Relay

BEHIND = ["behind/r1.yaml", "behind/r2.yaml", "chain/r3.yaml"]


def listening(port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return True
    except OSError:
        return False


class Records(logging.Handler):
    """Keeps what the ``netterm`` logger says, from any thread."""

    def __init__(self) -> None:
        super().__init__(logging.WARNING)
        self.lines: List[str] = []
        self.guard = threading.Lock()

    def emit(self, record: logging.LogRecord) -> None:
        with self.guard:
            self.lines.append(record.getMessage())

    def wait_for(self, text: str, seconds: float = 10) -> str:
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            with self.guard:
                found = [line for line in self.lines if text in line]
            if found:
                return found[0]
            time.sleep(0.1)
        raise AssertionError(f"nothing said {text!r}: {self.lines}")


class JumpHostsTest(BridgeTest):
    """The workspace of the tests below: hosts behind one jump host and
    behind a chain of two, all reaching the device-mode server."""

    def setUp(self) -> None:
        super().setUp()
        self.bastion = JumpServer()
        self.addCleanup(self.bastion.close)
        self.site = JumpServer()
        self.addCleanup(self.site.close)
        device = self.server.device_port
        lab = "credential_profile: lab\n"
        self.workspace.write("jumps/bastion.yaml", f"address: 127.0.0.1\nport: {self.bastion.port}\n{lab}")
        self.workspace.write(
            "jumps/site.yaml",
            f"address: 127.0.0.1\nport: {self.site.port}\njump_host: jumps/bastion.yaml\n{lab}",
        )
        self.workspace.write(
            "behind/_folder.yaml",
            f"jump_host: jumps/bastion.yaml\nplatform: cisco_ios\nport: {device}\n{lab}",
        )
        self.workspace.write("behind/r1.yaml", "address: 127.0.0.1\n")
        # The same device by another name, through the same jump host.
        self.workspace.write("behind/r2.yaml", "address: localhost\n")
        self.workspace.write(
            "chain/r3.yaml",
            f"address: 127.0.0.1\nport: {device}\njump_host: jumps/site.yaml\nplatform: cisco_ios\n{lab}",
        )
        key = self.server.device_key
        self.trust(
            f"[127.0.0.1]:{self.bastion.port} {self.bastion.key}",
            f"[127.0.0.1]:{self.site.port} {self.site.key}",
            f"[127.0.0.1]:{device} {key}",
            f"[localhost]:{device} {key}",
        )
        self.records = Records()
        logger = logging.getLogger("netterm")
        logger.addHandler(self.records)
        self.addCleanup(logger.removeHandler, self.records)

    def trust(self, *lines: str) -> None:
        """The app's known hosts, as if it had trusted these keys."""
        with open(self.workspace.data / "known_hosts", "a", encoding="utf-8") as known:
            for line in lines:
                known.write(line + "\n")

    def nornir(self, hosts: List[str]) -> Nornir:
        nr = InitNornir(
            runner={"plugin": "threaded", "options": {"num_workers": 3}},
            inventory={
                "plugin": "NettermInventory",
                "options": {"workspace": str(self.workspace.root), "hosts": hosts},
            },
            logging={"enabled": False},
        )
        # Connections first, then the plugin's tunnels, then the servers.
        self.addCleanup(close_all_tunnels)
        self.addCleanup(nr.close_connections)
        return nr


class Tunnels(JumpHostsTest):

    def test_a_nornir_task_logs_in_behind_one_and_two_jump_hosts(self) -> None:
        nr = self.nornir(BEHIND)
        ports = set()
        for name in BEHIND:
            host = nr.inventory.hosts[name]
            self.assertEqual(host.hostname, "127.0.0.1")
            self.assertNotEqual(host.port, self.server.device_port)
            ports.add(host.port)
            data = host["netterm"]
            self.assertTrue(data["behind_jump_host"])
            self.assertIsNone(data["tunnel_error"])
            self.assertEqual(data["tunnel"]["port"], host.port)
            # The device's own key is checked, against the app's.
            extras = host.get_connection_parameters("netmiko").extras
            self.assertTrue(extras["ssh_strict"])
            self.assertEqual(extras["alt_key_file"], data["tunnel"]["known_hosts"])
            known = Path(extras["alt_key_file"]).read_text(encoding="utf-8")
            self.assertIn(f"[127.0.0.1]:{host.port} {self.server.device_key}", known)
            self.assertIn(f"127.0.0.1 {self.server.device_key}", known)
        self.assertEqual(len(ports), 3)
        results = nr.run(task=netmiko_send_command, command_string="show hostname")
        self.assertEqual(set(results), set(BEHIND))
        for name, result in results.items():
            with self.subTest(host=name):
                self.assertFalse(result.failed, result[0].exception)
                self.assertIn(DEVICE_HOSTNAME, result[0].result)

    def test_a_device_key_other_than_the_trusted_one_is_refused(self) -> None:
        nr = self.nornir(["behind/r1.yaml"])
        host = nr.inventory.hosts["behind/r1.yaml"]
        # As if the device had been replaced since the app trusted it.
        Path(host["netterm"]["tunnel"]["known_hosts"]).write_text(
            f"[127.0.0.1]:{host.port} {self.bastion.key}\n", encoding="utf-8"
        )
        result = nr.run(task=netmiko_send_command, command_string="show hostname")["behind/r1.yaml"]
        self.assertTrue(result.failed)
        self.assertIn("does not match", str(result[0].exception))

    def test_hosts_that_cannot_be_tunnelled_carry_the_reason_and_the_rest_loads(self) -> None:
        stranger = JumpServer()
        self.addCleanup(stranger.close)
        self.workspace.write("open.yaml", f"address: 127.0.0.1\nport: {self.bastion.port}\n")
        self.workspace.write(
            "stranger.yaml", f"address: 127.0.0.1\nport: {stranger.port}\ncredential_profile: lab\n"
        )
        self.workspace.write("refused/no-login.yaml", "address: 127.0.0.1\njump_host: open.yaml\n")
        self.workspace.write("refused/broken.yaml", "address: 127.0.0.1\njump_host: gone.yaml\n")
        self.workspace.write("refused/new-device.yaml", "address: 192.0.2.7\njump_host: jumps/bastion.yaml\n")
        self.workspace.write(
            "refused/new-jump.yaml",
            f"address: 127.0.0.1\nport: {self.server.device_port}\njump_host: stranger.yaml\n",
        )
        loader = NettermInventory(str(self.workspace.root), credentials=False, tunnels=True)
        inventory = loader.load()
        self.addCleanup(loader.opened.close)

        def error(name: str) -> str:
            data = inventory.hosts[name]["netterm"]
            self.assertTrue(data["behind_jump_host"])
            self.assertIsNone(data["tunnel"])
            self.assertIsNotNone(data["tunnel_error"])
            return data["tunnel_error"]

        self.assertIn("open.yaml has no stored credential profile", error("refused/no-login.yaml"))
        self.assertIn("gone.yaml", error("refused/broken.yaml"))
        self.assertIn("connect to it once in the app", error("refused/new-device.yaml"))
        # An unknown jump-host key is named, and nothing is trusted.
        new_jump = error("refused/new-jump.yaml")
        self.assertIn("not trusted yet", new_jump)
        self.assertIn("SHA256:", new_jump)
        known = (self.workspace.data / "known_hosts").read_text(encoding="utf-8")
        self.assertNotIn(str(stranger.port), known)
        self.assertEqual(inventory.hosts["refused/no-login.yaml"].hostname, "127.0.0.1")
        # The others are tunnelled; the warning names only the refused ones.
        self.assertIsNotNone(inventory.hosts["behind/r1.yaml"]["netterm"]["tunnel"])
        self.assertIsNone(inventory.hosts["devices/r-pass.yaml"]["netterm"]["tunnel_error"])
        warning = self.records.wait_for("could not be tunnelled")
        self.assertIn("refused/broken.yaml", warning)
        self.assertNotIn("behind/r1.yaml", warning)

    def test_a_telnet_host_behind_a_jump_host_is_marked_and_says_why(self) -> None:
        self.workspace.write(
            "telnet/t1.yaml", "address: 192.0.2.5\nprotocol: telnet\njump_host: jumps/bastion.yaml\n"
        )
        for hosts in (
            # No SSH host behind a jump host: no tunnel is opened at all.
            ["telnet/t1.yaml", "devices/r-pass.yaml"],
            # With one: the binary refuses the telnet host itself.
            ["telnet/t1.yaml", "behind/r1.yaml"],
        ):
            with self.subTest(hosts=hosts):
                self.records.lines.clear()
                loader = NettermInventory(str(self.workspace.root), hosts=hosts, credentials=False, tunnels=True)
                inventory = loader.load()
                if loader.opened is not None:
                    self.addCleanup(loader.opened.close)
                host = inventory.hosts["telnet/t1.yaml"]
                data = host["netterm"]
                self.assertTrue(data["behind_jump_host"])
                self.assertIsNone(data["tunnel"])
                self.assertIn("only SSH is tunnelled", data["tunnel_error"])
                self.assertEqual(host.hostname, "192.0.2.5")
                warning = self.records.wait_for("could not be tunnelled")
                self.assertIn("telnet/t1.yaml", warning)
                self.assertIsNone(inventory.hosts[hosts[1]]["netterm"]["tunnel_error"])
        exported = {host["id"]: host for host in netterm.inventory(self.workspace.root)["hosts"]}
        with self.assertRaises(netterm.Unreachable) as refused:
            netterm.endpoint(exported["telnet/t1.yaml"], 23)
        self.assertIn("only SSH is tunnelled", str(refused.exception))

    def test_without_tunnels_hosts_behind_jump_hosts_say_so(self) -> None:
        inventory = NettermInventory(str(self.workspace.root), hosts=BEHIND, tunnels=False).load()
        host = inventory.hosts["behind/r1.yaml"]
        self.assertEqual(host.port, self.server.device_port)
        self.assertIn("tunnels off", host["netterm"]["tunnel_error"])

    def test_the_tunnel_ends_with_the_script_that_started_it(self) -> None:
        script = self.workspace.data / "starter.py"
        script.write_text(
            "import json, sys, time\n"
            "import netterm\n"
            "opened = netterm.tunnels(sys.argv[1], hosts=['behind/r1.yaml'])\n"
            "print(json.dumps(opened.hosts['behind/r1.yaml']['port']), flush=True)\n"
            "if sys.argv[2] == 'end':\n"
            "    sys.exit(0)\n"
            "time.sleep(600)\n",
            encoding="utf-8",
        )
        for how in ("end", "kill"):
            with self.subTest(how=how):
                starter = subprocess.Popen(
                    [sys.executable, str(script), str(self.workspace.root), how],
                    stdout=subprocess.PIPE,
                    text=True,
                )
                assert starter.stdout is not None
                port = json.loads(starter.stdout.readline())
                if how == "kill":
                    self.assertTrue(listening(port))
                    starter.kill()
                starter.wait()
                starter.stdout.close()
                until = time.monotonic() + 10
                while listening(port) and time.monotonic() < until:
                    time.sleep(0.1)
                self.assertFalse(listening(port), "the tunnel still listens")

    def test_a_jump_host_going_down_fails_its_hosts_with_a_reason(self) -> None:
        nr = self.nornir(["behind/r1.yaml"])
        first = nr.run(task=netmiko_send_command, command_string="show hostname")["behind/r1.yaml"]
        self.assertFalse(first.failed, first[0].exception)
        self.bastion.close()
        started = time.monotonic()
        again = nr.run(task=netmiko_send_command, command_string="show hostname")["behind/r1.yaml"]
        self.assertTrue(again.failed)
        self.assertLess(time.monotonic() - started, 30, "the script hung")
        said = self.records.wait_for("behind/r1.yaml")
        self.assertIn("jump host jumps/bastion.yaml was lost", said)

    def test_scrapli_checks_the_device_key_through_the_tunnel(self) -> None:
        try:
            from scrapli.driver import GenericDriver
        except ImportError:
            self.skipTest("scrapli is not installed")
        opened = netterm.tunnels(self.workspace.root, hosts=["behind/r1.yaml"])
        self.addCleanup(opened.close)
        tunnel = opened.hosts["behind/r1.yaml"]
        login = netterm.credentials(self.workspace.root, "behind/r1.yaml")
        driver = GenericDriver(
            comms_prompt_pattern=r"^\S+[#>] ?$",
            host=tunnel["address"],
            port=tunnel["port"],
            auth_username=login["username"],
            auth_password=login["auth"]["password"],
            auth_strict_key=True,
            ssh_known_hosts_file=tunnel["known_hosts"],
            transport="paramiko",
            timeout_ops=10,
        )
        driver.open()
        try:
            self.assertIn(DEVICE_HOSTNAME, driver.send_command("show hostname").result)
        finally:
            driver.close()


def banner(port: int) -> bytes:
    """The first bytes a service on ``127.0.0.1:port`` sends."""
    with socket.create_connection(("127.0.0.1", port), timeout=10) as connection:
        return connection.recv(4)


class TunnelPorts(JumpHostsTest):
    """Extra device ports (#93). The device's second port is a relay to
    its SSH service, so it shows the device's own host key; the plain test
    server's port, as a port of the same device, shows another key."""

    def setUp(self) -> None:
        super().setUp()
        self.mirror = Relay(self.server.device_port)
        self.addCleanup(self.mirror.close)
        #: A port of the device that shows a key the app never trusted.
        self.stranger = self.server.port

    def test_tunnels_report_the_loopback_port_of_each_named_port(self) -> None:
        device = self.server.device_port
        ports = [self.mirror.port, self.stranger, device, self.mirror.port]
        opened = netterm.tunnels(self.workspace.root, hosts=BEHIND, ports=ports)
        self.addCleanup(opened.close)
        loopbacks = set()
        for name in BEHIND:
            with self.subTest(host=name):
                entry = opened.hosts[name]
                extra = entry["extra_ports"]
                # The SSH port named again and a port named twice add nothing.
                self.assertEqual(set(extra), {self.mirror.port, self.stranger})
                self.assertNotIn(entry["port"], extra.values())
                loopbacks.update(extra.values())
                known = Path(entry["known_hosts"]).read_text(encoding="utf-8")
                for loopback in [entry["port"], *extra.values()]:
                    self.assertEqual(banner(loopback), b"SSH-")
                    self.assertIn(f"[127.0.0.1]:{loopback} {self.server.device_key}", known)
        self.assertEqual(len(loopbacks), 2 * len(BEHIND))

    def test_without_ports_an_entry_is_as_before(self) -> None:
        opened = netterm.tunnels(self.workspace.root, hosts=["behind/r1.yaml"])
        self.addCleanup(opened.close)
        self.assertEqual(
            set(opened.hosts["behind/r1.yaml"]), {"address", "port", "host_keys", "known_hosts"}
        )

    def test_a_port_that_is_not_a_port_opens_nothing(self) -> None:
        for bad in (0, 65536, -1, True, "830", 830.0):
            with self.subTest(port=bad):
                with self.assertRaises(ValueError):
                    netterm.tunnels(self.workspace.root, hosts=BEHIND, ports=[bad])
        with self.assertRaises(ValueError):
            netterm.tunnels(self.workspace.root, hosts=BEHIND, ports=830)

    def test_endpoint_says_where_to_connect_or_why_not(self) -> None:
        self.workspace.write("refused/new-device.yaml", "address: 192.0.2.7\njump_host: jumps/bastion.yaml\n")
        exported = {host["id"]: host for host in netterm.inventory(self.workspace.root)["hosts"]}
        named = self.mirror.port
        opened = netterm.tunnels(self.workspace.root, ports=[named])
        self.addCleanup(opened.close)
        r1 = exported["behind/r1.yaml"]
        entry = opened.hosts["behind/r1.yaml"]
        # A tunnelled host: its tunnel's listener for that port.
        self.assertEqual(netterm.endpoint(r1, named, opened), ("127.0.0.1", entry["extra_ports"][named]))
        self.assertEqual(opened.endpoint(r1, named), ("127.0.0.1", entry["extra_ports"][named]))
        address, port = opened.endpoint(r1, self.server.device_port)
        self.assertEqual((address, port), ("127.0.0.1", entry["port"]))
        # A direct host: its own address and the port, tunnels or not.
        direct = exported["devices/r-pass.yaml"]
        self.assertEqual(netterm.endpoint(direct, 830, opened), ("127.0.0.1", 830))
        self.assertEqual(netterm.endpoint(direct, 830), ("127.0.0.1", 830))
        self.assertEqual(netterm.endpoint(direct, 443).port, 443)
        # A host that could not be tunnelled: the tunnel's reason.
        with self.assertRaises(netterm.Unreachable) as refused:
            opened.endpoint(exported["refused/new-device.yaml"], named)
        self.assertIn("refused/new-device.yaml", str(refused.exception))
        self.assertIn("connect to it once in the app", str(refused.exception))
        # A port that was not named.
        with self.assertRaises(netterm.Unreachable) as unnamed:
            opened.endpoint(r1, 4444)
        self.assertIn("port 4444 was not named", str(unnamed.exception))
        # A host behind a jump host, and no tunnels.
        with self.assertRaises(netterm.Unreachable) as none:
            netterm.endpoint(r1, named)
        self.assertIn("no tunnels", str(none.exception))
        self.assertIsInstance(none.exception, netterm.NettermError)

    def test_the_nornir_option_opens_the_ports_and_the_host_data_answers(self) -> None:
        self.workspace.write("refused/new-device.yaml", "address: 192.0.2.7\njump_host: jumps/bastion.yaml\n")
        named = self.mirror.port
        loader = NettermInventory(
            str(self.workspace.root), credentials=False, tunnels=True, tunnel_ports=[named, 443]
        )
        inventory = loader.load()
        self.addCleanup(loader.opened.close)
        for name in BEHIND:
            with self.subTest(host=name):
                host = inventory.hosts[name]
                entry = loader.opened.hosts[name]
                # hostname and port stay the SSH endpoint.
                self.assertEqual((host.hostname, host.port), ("127.0.0.1", entry["port"]))
                answers = host["netterm"]["ports"]
                self.assertEqual(set(answers), {named, 443})
                self.assertEqual(answers[named], {"address": "127.0.0.1", "port": entry["extra_ports"][named]})
                self.assertEqual(banner(answers[named]["port"]), b"SSH-")
                # No connection plugin is given options for the extra ports.
                self.assertFalse({"ncclient", "scrapli_netconf"} & set(host.connection_options))
        direct = inventory.hosts["devices/r-pass.yaml"]["netterm"]["ports"]
        self.assertEqual(direct, {named: {"address": "127.0.0.1", "port": named}, 443: {"address": "127.0.0.1", "port": 443}})
        refused = inventory.hosts["refused/new-device.yaml"]["netterm"]["ports"][named]
        self.assertIn("connect to it once in the app", refused["error"])

    def test_a_script_that_names_no_ports_gets_no_answers(self) -> None:
        nr = self.nornir(BEHIND)
        for name in BEHIND:
            self.assertEqual(nr.inventory.hosts[name]["netterm"]["ports"], {})
            self.assertNotIn("extra_ports", nr.inventory.hosts[name]["netterm"]["tunnel"])
        self.assertEqual(nr.inventory.hosts["behind/r1.yaml"].port, nr.inventory.hosts["behind/r1.yaml"]["netterm"]["tunnel"]["port"])

    def open_ports(self) -> "tuple[dict, dict]":
        """Tunnels to r1 with the mirror and the stranger named, and r1's login."""
        opened = netterm.tunnels(self.workspace.root, hosts=["behind/r1.yaml"], ports=[self.mirror.port, self.stranger])
        self.addCleanup(opened.close)
        login = netterm.credentials(self.workspace.root, "behind/r1.yaml")
        return opened.hosts["behind/r1.yaml"], login

    def test_a_strict_key_client_through_an_extra_port_checks_the_device_key(self) -> None:
        entry, login = self.open_ports()
        for port, accepted in ((self.mirror.port, True), (self.stranger, False)):
            with self.subTest(port=port):
                client = paramiko.SSHClient()
                client.load_host_keys(entry["known_hosts"])
                client.set_missing_host_key_policy(paramiko.RejectPolicy())
                try:
                    connect = lambda: client.connect(  # noqa: E731
                        "127.0.0.1",
                        port=entry["extra_ports"][port],
                        username=login["username"],
                        password=login["auth"]["password"],
                        look_for_keys=False,
                        allow_agent=False,
                        timeout=10,
                    )
                    if accepted:
                        connect()
                        self.assertTrue(client.get_transport().is_authenticated())
                    else:
                        with self.assertRaises(paramiko.SSHException):
                            connect()
                finally:
                    client.close()

    def test_ncclient_checks_the_device_key_through_an_extra_port(self) -> None:
        try:
            from ncclient import manager
            from ncclient.transport.errors import SSHUnknownHostError
        except ImportError:
            self.skipTest("ncclient is not installed")
        entry, login = self.open_ports()
        options = netterm.host_key_options(entry)["ncclient"]
        self.assertTrue(options["hostkey_verify"])
        for port, accepted in ((self.mirror.port, True), (self.stranger, False)):
            with self.subTest(port=port):
                with self.assertRaises(Exception) as failed:
                    # The test device speaks no NETCONF: the session fails
                    # after the key check, or at it.
                    manager.connect(
                        host="127.0.0.1",
                        port=entry["extra_ports"][port],
                        username=login["username"],
                        password=login["auth"]["password"],
                        look_for_keys=False,
                        allow_agent=False,
                        timeout=10,
                        **options,
                    )
                self.assertEqual(isinstance(failed.exception, SSHUnknownHostError), not accepted, failed.exception)

    def test_scrapli_netconf_checks_the_device_key_through_an_extra_port(self) -> None:
        try:
            from scrapli_netconf.driver import NetconfDriver
        except ImportError:
            self.skipTest("scrapli_netconf is not installed")
        entry, login = self.open_ports()
        options = netterm.host_key_options(entry)["scrapli_netconf"]
        for port, accepted in ((self.mirror.port, True), (self.stranger, False)):
            with self.subTest(port=port):
                driver = NetconfDriver(
                    host="127.0.0.1",
                    port=entry["extra_ports"][port],
                    auth_username=login["username"],
                    auth_password=login["auth"]["password"],
                    transport="paramiko",
                    timeout_socket=10,
                    timeout_transport=10,
                    timeout_ops=10,
                    **options,
                )
                with self.assertRaises(Exception) as failed:
                    driver.open()
                driver.close()
                said = str(failed.exception)
                self.assertEqual("public key does not match" in said, not accepted, said)


if __name__ == "__main__":
    unittest.main()
