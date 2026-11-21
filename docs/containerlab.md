# Containerlab device suite

A slower, opt-in test suite that opens real sessions to network operating
systems on a containerlab server, through the core's public
API, the same way the app does (a workspace host, a credential profile,
`Profiles::profile_target`, host-key checks, session logs, jobs and config
backups with the built-in platform definitions). The topology has Nokia SR
Linux, Arista cEOS, FRR, Cisco IOS-XE (CSR 1000v) and Juniper cJunos
Evolved; any other device can be added to the lab file.

Every check passes against the topology's Nokia SR Linux 24.10.1, Arista
cEOS 4.36.0.1F, FRR 10.2.1, Cisco CSR 1000v (IOS-XE 17.03.08) and Juniper
cJunos Evolved 25.4R1.13-EVO (containerlab 0.83.0, through the containerlab
server as the jump host), first run 2026-11-14 (the platform checks and the
Cisco and Juniper nodes since #47, the files check since #108, first run
2026-11-21). The README's tested-platforms list comes from these runs.

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
| paging off                     | A job driven by the device's built-in platform definition runs `paging_command` (more lines than a job's 100-row terminal): no pager (`--More--`, `---(more)---`, ...) shows and the job ends at the prompt |
| config backup                  | A config backup job keeps the configuration without the prompt (its last line matches `config_end`); a second backup of the unchanged device keeps nothing new, so the definition's ignored lines cover what changes by itself |
| privileged mode                | Only where the definition has a privilege command: a job's `privilege_check` command shows `privilege_expect` (level 15) |
| fill in from devices           | Only with `NETTERM_LAB_PYTHON` (a Python with NAPALM): the app's script (`python -m netterm.enrich`, from `python/`) runs as a job for every device whose definition has a NAPALM driver (cEOS, IOS-XE, Junos), through the headless binary's tunnels (`netterm-cli tunnel`, opened by the package) when there is a jump host, after a terminal session through the chain has trusted the keys; every device reports vendor, model, serial number and OS version, and applying writes them |
| tunnel ports                   | Only with `NETTERM_LAB_PYTHON`: a script (`crates/core/tests/lab/tunnel_ports.py`) opens the headless binary's tunnels with ports 830 and 443 named, through the jump host, after a terminal session has trusted the keys; ncclient reaches NETCONF on the `junos` device's port 830 with the device's key checked against the keys the app trusts, gets the running configuration, and is refused with another key in its known-hosts file; an HTTPS eAPI call (requests) runs `show version` on the `ceos` device's port 443. A check whose library that Python lacks, or whose platform the lab file has no device of, is NOT CHECKED |
| second channel and backup on login | A session opened with the host's backup on login reports it saved; then a second shell channel on that session's connection is opened: accepted (the command runs there, the terminal is untouched) or refused (then the backup used a second connection) |
| files                          | What [Files on the lab devices](#files-on-the-lab-devices) says the device offers, on a file connection of its own (as a Files tab and a file job make one), with a 300 KiB file. SFTP: the platform's file folder (else the login's) is listed, the file is uploaded with its size checked, downloaded and compared, renamed and deleted. A device without SFTP: the fall back to SCP happens by itself, the file is sent to the platform's file folder, fetched and compared (an `ios` device takes it at `flash:` and gives it back from `bootflash:`, the same storage on the CSR 1000v); it stays on the device under the fixed name `netterm-lab-check.bin` (SCP cannot delete), replaced by each run. A device that lacks the settings of step 1 (`lab/configs/csr1-files.cfg`, `lab/configs/cjunos1-files.set`) is NOT CHECKED, with the device's words and the app's hint as the reason |

The output of every check (pass, fail or not checked, with the negotiated
algorithms) is printed, so run it with `--nocapture` (the npm script does).
A check that applies to no device still passes, but prints `NOT CHECKED on
any device`: with the example lab, the terminal-size check on SR Linux and
cEOS (no size command). The app does not tunnel telnet through a jump
host, so telnet is checked on cEOS, which the topology configures for
telnet and publishes on the server's port 2323; the lab file's
`telnet_address` sends the telnet check there directly. The topology's cEOS
config also turns on eAPI over HTTPS (`management api http-commands`,
`protocol https`) for the tunnel ports check; cJunos Evolved needs NETCONF on
830 turned on by hand after each deploy (step 1 below). When the output
fitted in the flow-control window, the large-output check says so, since
flow control was then not exercised.

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
   users. The CSR 1000v image (`vrnetlab/cisco_csr1000v:17.03.08`) is
   built with [vrnetlab](https://github.com/srl-labs/vrnetlab) from Cisco's
   qcow2, and cJunos Evolved (`cjunosevolved:25.4R1.13-EVO`) is loaded from
   Juniper's download; both are VMs (KVM, about 4 and 8 GB of memory) and
   take several minutes to boot after the deploy. Remove a node from the
   topology (and the lab file) if its image is not available. The FRR image
   must be rebuilt after `lab/frr/` changes (#47 added the `vtysh` user).

   cJunos Evolved gets no startup configuration from the topology and
   boots with a minimal one (host name `HOSTNAME`, SSH only, no root
   password, so nothing can be committed). Once it answers on SSH, log in
   as `admin` and set what the suite expects: the host name (the
   `junos` prompt is `admin@cjunos1>`), a root password (Junos commits
   nothing without one) and NETCONF over SSH on port 830 (the tunnel
   ports check and NAPALM's `junos` driver):

   ```text
   configure
   set system host-name cjunos1
   set system root-authentication plain-text-password
   set system services netconf ssh
   commit and-quit
   ```

   For file transfers (#96), two devices need more after each deploy,
   pasted by hand like the lines above (the topology gives neither a
   startup configuration): `lab/configs/cjunos1-files.set` on cjunos1
   (the SFTP server, and a route back to your own network for reaching it
   without the jump host) and `lab/configs/csr1-files.cfg` on csr1 (the
   SCP server and SSH bulk mode, then `write memory`). The other devices
   offer SFTP and SCP as the topology deploys them. What each offers is in
   [Files on the lab devices](#files-on-the-lab-devices).

   Docker inside a Proxmox LXC container needs two entries in the
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

   For `lab_enrichment` and `lab_tunnel_ports`, also
   `export NETTERM_LAB_PYTHON=<python>` (without it both print NOT CHECKED).
   That Python needs NAPALM for `lab_enrichment`, and ncclient and
   requests for `lab_tunnel_ports` (`pip install napalm` brings all three;
   else `pip install ncclient requests`). The package `netterm` comes from
   this repository's `python/` folder; nothing else is needed.

   `npm run test:lab` is `cargo build -p netterm-cli && `
   `cargo test -p netterm-core --test containerlab -- --ignored --nocapture --test-threads=1`.
   Add a test name (`lab_ssh_login_and_command`, `lab_host_keys`,
   `lab_wrong_password`, `lab_terminal_size`,
   `lab_large_output_with_flow_control`, `lab_session_log`, `lab_telnet`,
   `lab_paging`, `lab_config_backup`, `lab_privilege`,
   `lab_second_channel`, `lab_files`, `lab_enrichment`, `lab_tunnel_ports`) after `--test containerlab` to run one check.

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
    platform: srlinux # srlinux | ceos | frr | vtysh | ios | nxos | junos | other
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
`password_prompt`. For the platform checks: `definition` (the built-in
platform definition jobs drive the device with, written as the host's
`platform`; `""` for none, which skips those checks), `paging_command`,
`privilege_check` with `privilege_expect`, `config_end` (a regular
expression for the configuration's last line) and `enable_secret_env`
(the profile's enable secret, for a privilege command that asks for one).

## Platform defaults (verified on the versions above)

| Platform  | Username | Prompt             | Setup               | Command                    | Expect             | Large output                      | Size         | Exit   |
| --------- | -------- | ------------------ | ------------------- | -------------------------- | ------------------ | --------------------------------- | ------------ | ------ |
| `srlinux` | admin    | `A:srl1#`          |                     | `show version`             | `software version` | `info detail from state /`        | `bash stty size` | `quit` |
| `ceos`    | admin    | `ceos1>`/`ceos1#`  | `terminal length 0` | `show version`             | `arista`           | `show running-config all`         | `bash stty size` | `exit` |
| `frr`     | root     | ends in `$` or `#` |                     | `vtysh -c 'show version'`  | `FRRouting`        | `seq 1 100000`                    | `stty size`  | `exit` |
| `vtysh`   | vtysh    | `frr1#`            | `terminal length 0` | `show version`             | `FRRouting`        |                                   |              | `exit` |
| `ios`     | admin    | `csr1#`            | `terminal length 0` | `show version`             | `cisco ios`        | `show running-config all`         |              | `exit` |
| `nxos`    | admin    | `nxos1#`           | `terminal length 0` | `show version`             | `nx-os`            | `show running-config all`         |              | `exit` |
| `junos`   | admin    | `admin@cjunos1>`   | `set cli screen-length 0` | `show version`       | `junos`            |                                   |              | `exit` |
| `other`   | required | required           |                     | required                   | required           |                                   |              | required |

And for the platform checks (`nxos` not verified: no image yet):

| Platform  | Definition | Paging command                    | Privilege check                      | Config ends   |
| --------- | ---------- | --------------------------------- | ------------------------------------ | ------------- |
| `srlinux` | `srlinux`  | `info flat`                       |                                      | `set / ...`   |
| `ceos`    | `eos`      | `show running-config all`         | `show privilege`: level 15           | `end`         |
| `frr`     | none (root gets a shell) |                     |                                      |               |
| `vtysh`   | `frr`      | `show memory`                     |                                      | `end`         |
| `ios`     | `ios`      | `show running-config all`         | `show privilege`: level 15           | `end`         |
| `nxos`    | `nxos`     | `show running-config all`         |                                      |               |
| `junos`   | `junos`    | `show system processes extensive` |                                      | `set ...`     |

The device passwords are the images' own defaults for SR Linux, cEOS, the
CSR 1000v and cJunos Evolved (the topology does not change them; see the
containerlab documentation of each kind) and, for FRR (root and vtysh),
whatever `NETTERM_LAB_PASSWORD` was when the lab was deployed. None is
written in the repository.

## Built-in platforms on real devices

What the platform checks found (2026-11-14), per built-in definition:

| Definition | Device                         | Paging off | Privileged mode            | Whole configuration      | Second channel on one connection |
| ---------- | ------------------------------ | ---------- | -------------------------- | ------------------------ | -------------------------------- |
| `srlinux`  | SR Linux 24.10.1               | yes (never pages) | not applicable      | yes (640 lines)          | accepted                         |
| `eos`      | cEOS 4.36.0.1F                 | yes        | yes (level 15)             | yes, ends `end`          | accepted                         |
| `frr`      | FRR 10.2.1, vtysh login shell  | yes        | not applicable             | yes, ends `end`          | accepted                         |
| `ios`      | CSR 1000v, IOS-XE 17.03.08     | yes        | yes (level 15)             | yes, ends `end`          | **refused** (`ResourceShortage`): backup on login uses a second connection (profile logins only) |
| `junos`    | cJunos Evolved 25.4R1.13-EVO   | yes        | not applicable             | yes (as `show configuration \| display set \| count`) | accepted |
| `nxos`     | not checked (no NX-OS image on the lab server) | | | | unknown |

No built-in value had to change. One job fix came out of it: SR Linux's
prompt is two lines (`--{ running }--[  ]--`, then `A:srl1#`), and the
first one ended up at the end of every kept configuration; a config
backup now drops trailing lines that match the definition's ignored lines.
Privileged mode was reached with `enable` on accounts that are already
privilege 15; an enable password question on a real device is not
exercised (the in-process device-mode server covers it).

Each built-in definition's file folder (#102) comes from
[Files on the lab devices](#files-on-the-lab-devices): `eos` `/mnt/flash`
(cEOS), `junos` `/var/tmp` (cJunos), `ios` `flash:` (on the CSR 1000v it
works as well as `bootflash:`), `srlinux` and `frr` none (files belong
in the login folder). Not verified: `nxos`'s `bootflash:` (no NX-OS image)
and `flash:` on physical Catalyst hardware.

Not covered yet: NX-OS (needs an n9kv image), legacy-crypto devices,
public-key logins to the devices themselves other than through the agent.

## Files on the lab devices

What the file transfer prototype (#96, branch `prototype/96-files`, not
merged) found on 2026-11-18, with the accounts the lab uses and the lab
configuration of step 1 (`lab/configs/csr1-files.cfg`,
`lab/configs/cjunos1-files.set`). The numbers and texts below come from the
prototype's runs. Since #108 the suite's `files` check (`lab_files`) checks
what the table says each device offers, with the app's own code; it passed
on all five devices through the jump host on 2026-11-21, on a lab deployed
that day, and reported csr1 and cjunos1 as NOT CHECKED, each with its
reason, before their settings were pasted.

| Device | SFTP | SCP | What was configured | Login lands in | Files belong in | Account needs |
| ------ | ---- | --- | ------------------- | -------------- | --------------- | ------------- |
| srl1 (SR Linux 24.10.1) | yes | yes | nothing | `/home/admin` | the login folder | the image's `admin` (no other account tried) |
| ceos1 (cEOS 4.36.0.1F) | yes | yes | nothing | `/home/admin` | `/mnt/flash` | privilege 15: at privilege 1 SFTP is accepted and closed at once, and SCP answers `% The 'scp' command is only available in privileged mode.` |
| frr1 (FRR 10.2.1 image) | yes | **no**: the image has no `scp` program (`sh: scp: not found`, exit 127) | nothing | `/root` (`/home/vtysh` for `vtysh`, whose login shell is vtysh: SFTP still works) | the login folder | any account |
| csr1 (IOS-XE 17.03.08) | **no**: accepts the subsystem, then exits 0 and closes before SFTP's first reply | yes, once turned on (`Administratively disabled.` before) | `ip scp server enable`, `ip ssh bulk-mode` | not applicable | `bootflash:` (`flash:` also works) | privilege 15: privilege 1 gets `Privilege denied.` |
| cjunos1 (cJunos Evolved 25.4R1.13) | yes, once turned on (`subsystem request failed` before) | yes | `set system services ssh sftp-server`; a route back for reaching it directly | `/var/home/admin` | `/var/tmp` | any login class: `read-only` uploads and downloads, but cannot delete (`Permission denied`) |

Every device takes a file connection while a terminal session to it is
open. csr1 has five vty lines (`line vty 0 4`); with five terminal
sessions open it closes a sixth connection before sending its SSH banner
(OpenSSH: `kex_exchange_identification: Connection closed by remote
host`). An idle file connection with no channel holds one of those lines,
and csr1 closes the whole connection once an SCP transfer ends (cjunos1
runs several on one connection). It does the same when it closes the SFTP
channel it has no server for: the connection goes with the channel, which
the app first took for a lost connection in a file job (#108; it now goes
by the channel the device closed in order). No device dropped a file connection left
idle for 21 minutes with SSH keepalives every 30 seconds.

The lab's speeds are those of virtual machines and containers on one
server, not of real devices: the unregistered CSR 1000v is held to
1 Mbit/s (`show platform hardware throughput level`), which caps its
downloads at 0.12 MB/s, and srl1 takes uploads at about 0.3 MB/s with any
tool. The prototype's comparison with OpenSSH is on #96.
