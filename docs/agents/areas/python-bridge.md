# Python bridge

`python/`, the `enodix` package and its Nornir inventory plugin.

- `python/` — the Python bridge (#50): the pure-Python package `enodix`
  (3.10+, no dependencies; extra `nornir`), user doc `python/README.md`. The
  Nornir mapping below is the one piece of behaviour outside the core (an
  adapter: the core's export and logins are the data). It only runs the
  headless binary (`_cli.py`: `find_cli` = `ENODIX_CLI`
  (must be a file, else `CliNotFound`), else Windows
  `%LOCALAPPDATA%\<productName>\enodix-cli.exe` (NSIS `currentUser`
  default; a custom install dir is not found), else `PATH`; `run` checks
  `format` 1, errors `EnodixError` / `CliNotFound` / `CliFailed { status,
reason }` with the binary's `error:` prefix stripped; stdin/terminal are
  the binary's, so a vault prompt works). Versions (#127):
  `compare_version(cli)`, called by `run` and `Tunnels` before the first
  use of each binary path in a process, runs `<cli> --version` (30 s, stdin
  closed) and, when `python_version(<its version>)` (the PEP 440 rule of
  `build-wheel.mjs`) is not `PACKAGE_VERSION` (`_build.VERSION`), warns
  once with `enodix.VersionWarning` (a `UserWarning`) naming the binary,
  both versions and what to install; nothing refused (the `format` check
  still decides); silent in the source tree or when `--version` fails.
  Dashes (#127): `values(...)` puts `--` before the positional values only
  when one starts with `-`, `option(name, value)` gives `--name=<value>`
  only then, so every command line an older binary understood is
  unchanged (an older one refuses the dashed ones, exit 2, and the version
  warning says why); `Tunnels` puts its `--port`s before the `--`.
  `enodix.inventory(ws, folder=|
filter=|hosts=)`, `credentials(ws, id)`, `secret(ws, name)` → value;
  `tunnels(..., ports=)`, `endpoint`, `Endpoint`, `Unreachable` and
  `host_key_options` are in `tunnels.md`.
  `enodix.nornir.EnodixInventory` (entry point `nornir.plugins.inventory`
  `EnodixInventory`; options `workspace`, `folder|filter|hosts`,
  `credentials` (default true), `tunnels`, `tunnel_ports` (#93)),
  `build_inventory(export, logins, tunnelled, tunnel_failure,
tunnel_ports)`: host
  name = host id, hostname = address, port/platform resolved (platform as
  written); groups = folder paths with the parent folder as parent group,
  group data `tags` (own) + `description`; host data = custom fields, then
  set built-in fields (win over custom), `tags` (effective), `enodix`
  (`id, name, folder, folders, settings, login, jump_hosts (ids),
jump_chain_error, behind_jump_host, withheld, custom`); `tags`/`enodix`
  are reserved (and since #60 `tunnel`, `tunnel_error`, since #93 `ports` in `enodix`). Logins asked once per profile name (lower-cased) through
  `credentials` of its first host; any failure raises naming the host and
  profile (no partial inventory). Password → `username`/`password`; key →
  connection options `netmiko` (`use_keys, key_file, passphrase`),
  `scrapli` (`auth_private_key[_passphrase]`), `napalm` (`optional_args`
  same as netmiko); agent → `use_keys, allow_agent`; enable secret →
  `secret` / `auth_secondary`. Secrets never go into host data. Hosts
  behind jump hosts (`behind_jump_host` = jump hosts or a broken chain, or
  since #127 a telnet host with a jump host: `_tunnel._behind_any`) go
  through tunnels (#60, below); a telnet one gets `tunnel_error` =
  `TELNET_REASON` (the binary's words, "…only SSH is tunnelled") and is in
  the warning even when no tunnel is opened (before #127 it was unmarked,
  with no error, unless SSH hosts behind jump hosts made the binary refuse
  it); `endpoint` says the same. Workspace problems are warnings too.
  PyPI: `.github/workflows/distribute.yml` uploads the wheel taken out of the
  release's portable zip (Trusted Publishing, environment `pypi`; 0.7.0 and
  earlier would be refused for the classifier `Framework :: Nornir`, not on
  PyPI's list: `CLASSIFIERS` in build-wheel.mjs must hold only listed ones,
  and `twine check` does not catch that). Metadata 2.1 with `Project-URL`s
  from `REPOSITORY`. Same file there as in the app, so the version warning
  is what keeps a PyPI install honest.
  Wheel: `scripts/build-wheel.mjs` (node, no Python: zip written by hand,
  fixed times, `RECORD` hashes) → `<dist>-<version>-py3-none-any.whl`,
  version = Cargo workspace version as PEP 440 (`-beta.1` → `b1`),
  `enodix/_build.py` (only in the wheel: `VERSION`, `PRODUCT_NAME` from
  `tauri.conf.json`, `CLI_NAME` from `crates/cli` `[[bin]]`); exports
  `wheelName`, `buildWheel`. `beforeBuildCommand` builds it into
  `target/release/python/`; the release installs it as `python/<wheel>`
  beside the app and puts it in the portable zip. Tests: `npm run
test:python` (`scripts/test-python.mjs`, NOT in `npm run check`: it
  needs Python and, once, the network): Python ≥3.10 from
  `ENODIX_TEST_PYTHON` / `py -3` / `python3` / `python`, else "SKIPPED"
  and exit 0; builds `enodix-cli` + the `test_ssh_server` example
  (debug), the wheel, a venv `target/python-test/venv-<x.y>` (nornir,
  nornir-netmiko, ncclient (#93) from the index if missing; wheel force-reinstalled), then
  `unittest discover python/tests` from `target/python-test` (imports the
  installed wheel) with `ENODIX_CLI`, `ENODIX_TEST_SSH_SERVER`.
  `tests/support.py`: each test copies `tests/fixtures/workspace` (id
  `python-bridge-test`), sets its own `ENODIX_DATA_DIR` and a
  `enodix-test-<hex>` `ENODIX_KEYCHAIN_PREFIX`, starts the example with
  `0 --device 0 --credential-agent <data> python-bridge-test` and writes
  `devices/_folder.yaml` (profile `lab`, `cisco_ios`, the device port).
  The example's `--credential-agent <data folder> <scope>` serves
  `lab` (password + enable secret), `keys` (the device server's client key
  written to `<data>/client_key`, passphrase), `agent`, secret
  `netbox-token` from a `MemoryStore` via `AgentServer`, printed as
  `credential agent: <json>`. The test shell now takes LF and CR LF as
  Enter too (Netmiko sends LF). `test_end_to_end.py`: `netmiko_send_command
show hostname` on `r-pass` (password) and `r-key` (key) through
  `InitNornir`. #127 tests: `test_find_cli.py` (`python_version`, a
  patched `PACKAGE_VERSION` warned once over two runs, the right one not
  at all, `values`/`option`), `test_inventory.py` `DashedNames` (folder
  `-old`, host `-spare.yaml` with its login, a workspace `-lab` by relative
  name, secret `-ro` looked for: status 1), `test_tunnels.py`
  `test_a_telnet_host_behind_a_jump_host_is_marked_and_says_why` (with no
  SSH host behind a jump host, then with one). Verified on Python 3.15 and 3.11 (not 3.10 itself); the
  agent profile is never used for a real login (it would use the user's
  agent). Not verified: Linux/macOS (no default location there: `PATH`),
  scrapli and NAPALM with these options, installing from the installed
  app's folder.
