# Tunnels for scripts

How scripts reach hosts behind jump hosts: the core, `enodix-cli tunnel` and the Python side.

- Tunnels for scripts (#60, `src/tunnel.rs`, `session/ssh.rs`,
  `crates/cli/src/tunnel.rs`, `python/enodix/_tunnel.py`; prototype #59
  on the branch `prototype/59-tunnel`, not merged). `tunnel::plan(ws,
&Profiles, &KnownHosts, ids, ports)` connects to nothing → `TunnelPlan { hosts:
[Planned::Ready(TunnelRequest { host, address, port, ports (extra device
ports, #92: sorted, without `port`, a port named twice once, 0 dropped),
jump_hosts, host_keys, connect_timeout (the device's: `forward` waits that
long) })
| Planned::Refused { host, reason }] }`, only hosts behind
  jump hosts (a broken chain counts; serial and direct left out), in the
  given order. Refused: telnet ("only SSH is tunnelled"), broken chain,
  a hop with `SshAuth::Prompt` (no stored profile), profile errors, and
  **a device whose key the app has not trusted** (`KnownHosts::trusted(
host, port)`, new; empty → "connect to it once in the app and trust and
  remember its key": jobs refuse unknown keys too, so nothing is checked
  off). `host_keys` = `<type> <base64>`. `tunnel::behind_jump_hosts(ws,
ids)` (no store). `Tunnels::open(plan, known_hosts, TunnelOptions {
buffer_limit (DEFAULT_BUFFER_LIMIT 8 MiB) }, report).await` (current
  runtime) connects each distinct chain (key = jump host names) once, in
  parallel, **before** listening, via `session::open_jump_chain_unattended`
  (askers with nobody behind: a key question → refused with "its <type>
  key SHA256:… (at h port p) is not trusted yet | not the one trusted
  for it"; a login question → "it asked …"); then, per host whose chain
  connected, one `127.0.0.1:0` listener for its SSH port and one per extra
  port (each its own `serve` task with its device port). `status()` →
  `[TunnelStatus { host, status: listening { address, port (the SSH
port's), host_keys, extra_ports: BTreeMap<device port, loopback port>
(skipped in the JSON when empty, so no ports = today's JSON) } | refused {
reason } }]` (snake_case, the CLI's JSON). Extra ports change no refusal
  and are not probed: nothing reaches the device until a program connects.
  Each accepted connection: the chain
  (reconnected, `TunnelEvent::ChainConnected|ChainFailed`, if
  `JumpConnections::lost()`), `forward` (`direct-tcpip` from the last
  hop to the listener's device port, the device's connect timeout), then
  `pump`: the channel's read half is
  drained at once into an unbounded queue counted against `buffer_limit`
  (over it → that connection closed, `Failed` "did not read its
  output"), so one stalled reader never blocks russh's shared connection
  (the quirk above; `tests/tunnels.rs` reproduces it with a paused
  terminal session through a tunnel and fails without the drain). A
  connection ending while its chain is lost → `Failed { host, reason:
"the connection to the jump host X was lost" }`. On an extra port every
  `Failed` reason starts `port <n>: ` (a closed port: "port 830: the jump
  host X could not reach it (…)"); the SSH port's reasons are unchanged. Dropping `Tunnels`
  aborts the listeners and their connections. `ssh.rs`: `connect_jumps`
  (extracted from `open_shell`), `JumpConnections { names, connections }`
  (`forward`, `lost`), `open_jump_chain`. CLI: `enodix-cli tunnel <ws>
[--folder|--filter|--host...] [--port <n>...]` (the inventory's limits
  and job scope, `workspace_and_limit` shared, after `tunnel::ports` has
  taken out the `--port`s: digits only, 1–65535, else usage error before
  anything opens; opens the credential store only if a host is behind a
  jump host) prints one line `{"format":1,"tunnels": [...]}` (a listening
  entry gains `"extra_ports": {"830": <loopback port>, …}` when ports were
  named), reports events on stderr as `enodix-cli: tunnel: …`, and
  serves until **stdin reaches EOF** (exit 0; the caller must hold stdin
  open). Python: `enodix.tunnels(ws, folder=|filter=|hosts=,
ports=)` → `Tunnels` (`hosts`: id → `{address, port, host_keys,
known_hosts}` (+ `extra_ports` {int device port: loopback port} only
  when ports were named, #93) | `{error}`; `ports` checked by
  `ports_of` (ints 1–65535, not bool, not a bare int/str: ValueError
  before anything starts) and passed as `--port`s; one known-hosts file
  per host in a temp folder with `127.0.0.1 <key>` and
  `[127.0.0.1]:<loopback> <key>` for the SSH port's listener and every
  extra one, as scrapli's paramiko transport ignores the port, plus
  `<known_hosts>_ssh_config` (`Host *` + quoted `UserKnownHostsFile`,
  for ncclient); stderr lines → `enodix` logger warnings; `close()`,
  context manager, atexit). `enodix.endpoint(host, port, tunnels=None)`
  / `Tunnels.endpoint` (#93; `_tunnel.answer` underneath): host = an
  exported host; → `Endpoint(address, port)` (NamedTuple): direct host
  (not `behind`, not telnet with a `jump_host` setting) → its address
  and the port; tunnelled → the SSH port's listener for its own
  `settings.port`, else `extra_ports[port]`; raises `Unreachable`
  (`EnodixError`) naming the host: no tunnels / not among them / the
  tunnel's refusal / "port N was not named"; serial → Unreachable.
  `host_key_options(tunnel)` (exported as `enodix.host_key_options`)
  adds `ncclient {hostkey_verify, ssh_config}` and `scrapli_netconf`
  (as scrapli). Nornir plugin option
  `tunnels` (default = `credentials`): `load` opens them with its own
  limit when any exported host is behind a jump host; tunnelled hosts
  get hostname `127.0.0.1`, the port, `enodix.tunnel {address, port,
known_hosts}` and `host_key_options` for `SSH_PORT_LIBRARIES` only
  (netmiko `ssh_strict, system_host_keys False, alt_host_keys,
alt_key_file`; scrapli `auth_strict_key, ssh_known_hosts_file`; napalm
  optional_args as netmiko; never ncclient/scrapli_netconf); the rest keep
  their address with `enodix.tunnel_error`, and the one warning names
  only those. Option `tunnel_ports` (#93, checked in `__init__`):
  passed to `tunnels(ports=)`; every host's `enodix.ports` = {port:
  {address, port} | {error}} from `answer` (`{}` without ports);
  hostname/port stay the SSH endpoint. Enrichment (`enrich.py`) tunnels the
  job's hosts behind jump hosts (`driver_arguments(..., tunnel)`; key
  options for `NETMIKO_DRIVERS` ios/nxos_ssh/eos only: eos is run over
  SSH there, which goes through Netmiko). Jobs: script and
  enrich pre-flight `tunnelled()` skips `JumpChain`, `BehindJumpHost`
  (no stored profile; message "the script's tunnel cannot log in to
  it"), `TelnetBehindJumpHost` (scripts only; enrich is `NotSsh` first).
  Tests: core `tests/tunnels.rs` (one hop, two, shared connection,
  refusals before connecting (with ports named too: no listener),
  untrusted/changed jump key named and not remembered, OTP jump host,
  unreachable device, chain cut → reason + reconnect, drop closes ports,
  stalled reader, buffer limit; extra ports (#92): one hop and two to a
  `support::tcp::TcpService` (echo or flood) beside the device, SSH port
  and duplicates add nothing, shared connection across ports, a closed
  port fails only that connection naming the port, chain cut on an extra
  port, drop closes every listener, stalled reader and buffer limit on
  an extra port; support `TestSshServer::cut_connections()` disconnects
  every connection),
  `script_jobs.rs`, `enrichment.rs`; cli `tests/tunnel.rs` (empty list
  opens no store, refusals incl. an unreachable jump host, limits, job
  scope, usage; `JumpHost`, a minimal russh jump host in the test (the
  fixture RSA key, password login, `direct-tcpip` only), for listening
  entries: the JSON without ports exactly as before, `--port` repeated →
  `extra_ports` per host and an echo service reached through one, bad
  values → exit 2 with nothing printed; russh is pinned once, in the
  root `Cargo.toml`'s `[workspace.dependencies]`); Python
  `tests/test_tunnels.py` (Nornir + Netmiko through one and two hops
  with strict keys, wrong device key refused, refusals in the data,
  tunnels off, the tunnel ends with its starting script (end and kill),
  jump host killed mid-run → fails fast with the logged reason; scrapli
  only if installed) and `test_enrich.py`; tunnel ports (#93, `TunnelPorts` in
  `test_tunnels.py`): `extra_ports` per host and known-hosts lines,
  entry unchanged without ports, bad ports, `endpoint` for tunnelled,
  direct, refused, unnamed and no-tunnels, the Nornir option's
  `enodix.ports` (and no ncclient/scrapli_netconf options), a strict
  paramiko client, ncclient (installed by `npm run test:python`) and
  scrapli_netconf (skipped unless installed) through an extra port:
  accepted where `support.Relay` mirrors the device's SSH service,
  refused on the plain server's port (another key); the test SSH server
  now refuses subsystem requests (`channel_failure`) so ncclient fails
  fast after the key check; `JumpServer` in `support.py`;
  lab `lab_tunnel_ports` (`tests/lab/tunnel_ports.rs` + `.py`, see
  `testing.md`): passes through the lab's jump host on 2026-11-18,
  ncclient 0.7.1 to cJunos Evolved 25.4 on 830 (trusted ssh-ed25519
  checked, an Ed25519 stranger refused) and eAPI on cEOS 4.36's 443 with
  requests; ncclient with only an RSA key known asks for `ssh-rsa` by
  name and cannot negotiate with Junos 25.4 (rsa-sha2 only): README limit;
  the example server prints `host key:` / `device host key:`. Known
  limits: since #129 job pre-flight warns of a script or enrich host
  behind jump hosts whose device or jump-host key is not trusted yet
  (`JobPlan::check_host_keys`, `jobs.md` "Host keys at pre-flight": the
  same lookups as `tunnel::plan` and the chain's connection, "the
  script's tunnel will refuse this host"); it still runs and is refused
  by the tunnel at run time (`tunnel_error` / the host's facts error),
  and a changed key is seen only then; chains are shared
  only when identical (hosts behind `A` and behind `A > B` each connect
  `A`); telnet behind jump hosts unreachable for scripts; NAPALM junos/
  iosxr drivers (and eos over eAPI) do not check the device key through
  a tunnel (PyEZ takes no key through it); other device ports (NETCONF
  830, eAPI 443, any TCP port) go through only when named (`--port`,
  `ports=`, `tunnel_ports`), for every host of the call, not per host
  and not added to running tunnels; a named port is not probed (a closed one
  fails at connect time); the app checks no TLS certificate through a
  tunnel (the script connects to 127.0.0.1, so names cannot match); any
  local process can use a tunnel's ports while it runs (as `ssh -L`);
  jump-host secrets stay in the tunnel process's memory; a script that
  hands its stdin pipe to children keeps the tunnel alive until they
  exit; Linux/macOS lifetime and a silent (no RST) jump-host drop not
  observed (keepalives should catch it after ~3 connect timeouts).
