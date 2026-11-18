"""The lab check of tunnel ports (crates/core/tests/lab/tunnel_ports.rs).

    python tunnel_ports.py <workspace> <netconf host id or -> <eapi host id or ->

Opens the workspace's tunnels with ports 830 and 443 named, as a script
would (``netterm.tunnels(..., ports=[830, 443])``), and checks:

- netconf: ncclient reaches the host's NETCONF port (830) through its
  tunnel with the device's host key checked against the keys the app
  trusts (``netterm.host_key_options``), gets the running configuration,
  and is refused when the known-hosts file holds another key;
- eapi: an HTTPS eAPI call (``requests``) to the host's port 443 through
  its tunnel runs ``show version``.

Prints one JSON line per check: ``{"check", "host", "result" (passed,
failed or not checked), "detail"}``.
"""

from __future__ import annotations

import json
import sys
import tempfile
import traceback
from pathlib import Path
from typing import Any, Dict

import netterm


def say(check: str, host: str, result: str, detail: str) -> None:
    print(json.dumps({"check": check, "host": host, "result": result, "detail": detail}), flush=True)


def netconf(workspace: str, host: Dict[str, Any], opened: netterm.Tunnels) -> None:
    try:
        from ncclient import manager
        from ncclient.transport.errors import SSHUnknownHostError
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric import ed25519
    except ImportError as error:
        say("netconf", host["id"], "not checked", f"the lab Python has no ncclient ({error})")
        return
    address, port = opened.endpoint(host, 830)
    tunnel = opened.hosts[host["id"]]
    trusted = ", ".join(sorted({key.split()[0] for key in tunnel["host_keys"]}))
    login = netterm.credentials(workspace, host["id"])
    common = dict(
        host=address,
        port=port,
        username=login["username"],
        password=login["auth"].get("password"),
        look_for_keys=False,
        allow_agent=False,
        timeout=60,
        device_params={"name": "junos"},
    )
    try:
        session = manager.connect(**common, **netterm.host_key_options(tunnel)["ncclient"])
    except Exception as error:
        say("netconf", host["id"], "failed", f"keys trusted: {trusted}; {type(error).__name__}: {error}")
        return
    with session:
        reply = session.get_config(source="running")
        configuration = reply.data_xml
        if "<configuration" not in configuration:
            say("netconf", host["id"], "failed", f"get-config returned no configuration: {configuration[:300]}")
            return
        version = session.server_capabilities
        said = f"{address}:{port} serves device port 830 (key checked, trusted: {trusted}), session {session.session_id}, " + (
            f"{len(list(version))} capabilities, configuration of {len(configuration)} bytes"
        )
    # The same connection with a known-hosts file of another key: an
    # Ed25519 one, as ncclient asks the device for the known key's type by
    # its name, and for an RSA key that is ssh-rsa (SHA-1), which current
    # devices refuse before any key is shown.
    with tempfile.TemporaryDirectory() as folder:
        other = ed25519.Ed25519PrivateKey.generate().public_key()
        line = other.public_bytes(serialization.Encoding.OpenSSH, serialization.PublicFormat.OpenSSH).decode()
        known = Path(folder) / "known_hosts"
        known.write_text(f"{address} {line}\n[{address}]:{port} {line}\n", encoding="utf-8")
        config = Path(folder) / "ssh_config"
        config.write_text(f'Host *\n  UserKnownHostsFile "{known}"\n', encoding="utf-8")
        try:
            with manager.connect(**common, hostkey_verify=True, ssh_config=str(config)):
                say("netconf", host["id"], "failed", f"{said}; but another key was accepted too")
                return
        except SSHUnknownHostError as error:
            said += f"; with another key refused ({error})"
    say("netconf", host["id"], "passed", said)


def eapi(workspace: str, host: Dict[str, Any], opened: netterm.Tunnels) -> None:
    try:
        import requests
        import urllib3
    except ImportError as error:
        say("eapi", host["id"], "not checked", f"the lab Python has no requests ({error})")
        return
    address, port = opened.endpoint(host, 443)
    login = netterm.credentials(workspace, host["id"])
    # The certificate is the device's own, self-signed, for its own name:
    # through a tunnel the name cannot match (see the package's README).
    urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
    answer = requests.post(
        f"https://{address}:{port}/command-api",
        json={
            "jsonrpc": "2.0",
            "method": "runCmds",
            "params": {"version": 1, "cmds": ["show version"], "format": "json"},
            "id": "netterm-lab",
        },
        auth=(login["username"], login["auth"].get("password") or ""),
        verify=False,
        timeout=60,
    )
    answer.raise_for_status()
    body = answer.json()
    if "result" not in body:
        say("eapi", host["id"], "failed", f"eAPI answered {json.dumps(body)[:300]}")
        return
    version = body["result"][0]
    say(
        "eapi",
        host["id"],
        "passed",
        f"{address}:{port} serves device port 443: {version.get('modelName')} {version.get('version')}",
    )


def main() -> None:
    workspace, netconf_host, eapi_host = sys.argv[1:4]
    hosts = {host["id"]: host for host in netterm.inventory(workspace)["hosts"]}
    with netterm.tunnels(workspace, ports=[830, 443]) as opened:
        for check, host_id, run in (("netconf", netconf_host, netconf), ("eapi", eapi_host, eapi)):
            if host_id == "-":
                continue
            try:
                run(workspace, hosts[host_id], opened)
            except Exception as error:  # noqa: BLE001 - reported, not raised
                detail = f"{type(error).__name__}: {error}"
                say(check, host_id, "failed", detail + "\n" + traceback.format_exc(limit=3))


if __name__ == "__main__":
    main()
