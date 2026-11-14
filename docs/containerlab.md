# Containerlab device suite

A slower, opt-in test suite that opens real sessions to network operating
systems on a containerlab server, through the core's public
API, the same way the app does (a workspace host, a credential profile,
`Profiles::profile_target`, host-key checks, session logs). The spec names
Nokia SR Linux, Arista cEOS and FRR; any other device can be added to the
lab file.

**Status:** every check passes against the topology's Nokia SR Linux
24.10.1, Arista cEOS 4.36.0.1F and FRR 10.2.1 (containerlab 0.83.0, through
the containerlab server as the jump host), first run 2026-11-14. The README's
tested-platforms list comes from these runs.

## What it checks, per device

| Check                          | What passes                                                                                                                                      |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| ssh login and command          | Log in with the device's profile, get the prompt, run `command`, its output matches `expect`, `exit` ends the session from the device's side      |
| host keys                      | The first connection asks about the device's host key (and the jump host's); the next connection, same store, asks nothing                       |
| wrong password                 | A wrong password ends in "authentication failed". Each run makes one failed login per device: mind lockout policies                              |
| terminal size                  | `size_command` prints the size the session opened with, and the new size after a resize (only where `size_command` is set)                       |
| large output with flow control | `large_command` prints at least `large_min_bytes`; while the reader stalls for 3 s, no more than the flow-control window (256 KiB + a batch) arrives |
| session log                    | The session's log file holds the command output as plain text (no control sequences)                                                             |
| telnet                         | Log in by telnet (answers typed into the terminal), run `command`; only where `telnet` is set, and behind a jump host only with `telnet_address` (telnet is not tunnelled) |

The output of every check (pass, fail or not checked, with the negotiated
algorithms) is printed, so run it with `--nocapture` (the npm script does).
A check that applies to no device still passes, but prints `NOT CHECKED on
any device`: with the example lab, the terminal-size check on SR Linux and
cEOS (no size command). Telnet is checked on cEOS, which the topology
configures for telnet and publishes on the server's port 2323, because the
app does not tunnel telnet through a jump host; the lab file's
`telnet_address` sends the telnet check there directly. The large-output check says when the
output fitted in the flow-control window, so flow control was not exercised.

A prompt counts once the output ends with it and nothing more arrives for
300 ms, so output that briefly ends in `#` or `>` is not taken for one.
Device names may not start with `_` or be `clab-server` (the suite's own
files).

## Running it

1. Deploy the lab on the containerlab server (you do this; the suite never
   deploys anything). The repository's topology is
   `lab/netterm-test.clab.yml`, with startup configs under `lab/configs/`.
   FRR needs an image with an SSH server, built from `lab/frr/`:

   ```sh
   docker build -t netterm-frr:latest lab/frr
   NETTERM_LAB_PASSWORD=<frr root password> sudo -E containerlab deploy -t lab/netterm-test.clab.yml
   ```

   cEOS (`ceos:4.36.0.1F` in the topology) must be imported into Docker
   first (`docker import`), as Arista distributes it only to registered
   users. Remove a node from the topology (and the lab file) if its image
   is not available.

   **Docker inside a Proxmox LXC container** needs two entries in the
   container's `/etc/pve/lxc/<id>.conf` on the Proxmox host (besides
   `features: nesting=1` and `lxc.apparmor.profile: unconfined`), then a
   restart of the container:

   ```ini
   # Docker sees AppArmor as off, so it never runs apparmor_parser (which
   # fails without cap_mac_admin: "docker-default profile could not be
   # loaded", exit 243, for docker build and containerlab's -netns sidecar).
   lxc.mount.entry: /dev/null sys/module/apparmor/parameters/enabled none bind,optional 0 0
   # Keep sys_time and sys_rawio, which SR Linux's sr_device_mgr needs
   # (without them SR Linux never starts its management server or SSH).
   lxc.cap.drop:
   lxc.cap.drop: mac_admin mac_override sys_module
   ```

2. Write a lab file, outside git: copy `lab/lab.example.yaml` to
   `lab/lab.yaml` (ignored by git) or anywhere else, and fill in the
   server's address and username and each device's management address.

3. Set the environment and run, from the repository root:

   ```sh
   export NETTERM_LAB_CONFIG=/path/to/lab.yaml   # not needed for lab/lab.yaml
   export NETTERM_LAB_SRL_PASSWORD=...           # whatever password_env names
   export NETTERM_LAB_CEOS_PASSWORD=...
   export NETTERM_LAB_PASSWORD=...               # default for devices without password_env
   npm run test:lab
   ```

   `npm run test:lab` is
   `cargo test -p netterm-core --test containerlab -- --ignored --nocapture --test-threads=1`.
   Add a test name (`lab_ssh_login_and_command`, `lab_host_keys`,
   `lab_wrong_password`, `lab_terminal_size`,
   `lab_large_output_with_flow_control`, `lab_session_log`, `lab_telnet`)
   after `--test containerlab` to run one check.

Without a lab file the suite prints `containerlab suite SKIPPED: no lab
configured ...` and passes. A lab file that exists but cannot be used (a
missing password variable, an unknown key, `NETTERM_LAB_CONFIG` naming a
missing file) fails. `npm run check` and `cargo test` never run the lab
tests (they are `#[ignore]`d); they do run the suite's own tests, which
check the lab file parsing and run every check against the in-process
servers.

## The lab file

```yaml
timeout: 30 # seconds to wait for each answer from a device
jump_host: # optional: the containerlab server, to reach the devices through
  address: 192.0.2.10
  port: 22
  username: root
  # logs in with the SSH agent's keys (Windows OpenSSH agent or Pageant)
  # unless key_file (+ passphrase_env) or password_env is given
devices:
  - name: srl1 # letters, digits, - and _
    platform: srlinux # srlinux | ceos | frr | other
    address: 172.20.30.11
    port: 22
    password_env: NETTERM_LAB_SRL_PASSWORD # or agent: true
```

Passwords can also be written as `password:`, but keep them in environment
variables. Every check setting can be overridden per device: `username`,
`prompt` (a regular expression matched against the end of the output, with
control sequences removed), `setup` (lines sent after login), `command`,
`expect` (a regular expression), `large_command`, `large_min_bytes`,
`size_command`, `size_output` (with `{cols}` and `{rows}`), `exit`,
`telnet` (`true` or a port), `telnet_address` (where telnet is reached
directly, not through the jump host: for the topology's cEOS, the
containerlab server's address with `telnet: 2323`), `login_prompt`,
`password_prompt`.

## Platform defaults (verified on the versions above)

| Platform  | Username | Prompt             | Setup               | Command                    | Expect             | Large output                      | Size         | Exit   |
| --------- | -------- | ------------------ | ------------------- | -------------------------- | ------------------ | --------------------------------- | ------------ | ------ |
| `srlinux` | admin    | `A:srl1#`          |                     | `show version`             | `software version` | `info detail from state /`        | `bash stty size` | `quit` |
| `ceos`    | admin    | `ceos1>`/`ceos1#`  | `terminal length 0` | `show version`             | `arista`           | `show running-config all`         | `bash stty size` | `exit` |
| `frr`     | root     | ends in `$` or `#` |                     | `vtysh -c 'show version'`  | `FRRouting`        | `seq 1 100000`                    | `stty size`  | `exit` |
| `other`   | required | required           |                     | required                   | required           |                                   |              | required |

The device passwords are the images' own defaults for SR Linux and cEOS
(the topology does not change them; see the containerlab documentation of
each kind) and, for FRR, whatever `NETTERM_LAB_PASSWORD` was when the lab was
deployed. None is written in the repository.

Not covered yet: Cisco and Juniper images (licences), legacy-crypto devices,
public-key logins to the devices themselves other than through the agent.
