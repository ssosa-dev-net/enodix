# Roadmap

What comes next for enodix, what is not yet verified, and what is not
planned. What each release added is in [CHANGELOG.md](CHANGELOG.md).

## Principles

- **Daily use drives the order.** Features are built in the order they are
  needed in real network work, since that is what can be verified.
- **Local and trusted.** The app holds some of the most sensitive passwords
  an organisation has, so it stays on your computer: no telemetry, no
  accounts, no cloud. The only connection it makes on its own is the update
  check, which can be turned off; [docs/trust.md](docs/trust.md) lists every
  connection and what is kept where.
- **Solid before wide.** Hardening what exists comes before most new
  features.
- **Python is optional.** Everything except script jobs and Fill in from
  devices works without it.
- **Windows and Linux.** Both are supported; the core has no Windows-only
  assumptions.

## Next

- **Import from SecureCRT, PuTTY and KiTTY, and MobaXterm**
  ([#138](https://github.com/ssosa-dev-net/enodix/issues/138),
  [#139](https://github.com/ssosa-dev-net/enodix/issues/139),
  [#140](https://github.com/ssosa-dev-net/enodix/issues/140)).
- **Locking the vault** on idle, when the computer locks and on demand
  ([#96](https://github.com/ssosa-dev-net/enodix/issues/96)).
- **An update checked end to end:** a copy of 0.6.0 updating itself to the
  next release.
- **An independent security review.** An internal review was done in
  November 2026 and its findings fixed; nobody outside the project has
  reviewed the code yet.

## Not verified yet

These need hardware, devices or setups the project does not have.

- **Linux beyond the checks so far.** Linux is supported; the checks run in
  CI, under WSL (XFCE and GNOME sessions, and Fedora in a container) and in a
  cloud container; see [docs/dev/linux.md](docs/dev/linux.md). Not checked: a
  Linux desktop that is not WSL, a USB serial adapter, a native GPU driver,
  KDE, SELinux on a real Fedora.
- **Real devices.** Everything is checked against virtual devices in a
  containerlab lab ([docs/dev/containerlab.md](docs/dev/containerlab.md)): SR Linux,
  cEOS, FRR, IOS-XE (CSR 1000v) and cJunos Evolved. Not checked: Cisco NX-OS
  (its platform definition is a handful of values you can correct in the
  app), the flash names of physical Catalyst switches, transfer speeds on real
  hardware, a jump host that asks for a typed login.

## Later, undated

- NetBox sync, read-only and one-way, as something beside the app rather
  than built in.
- Temporary TFTP, FTP and SFTP servers serving a chosen folder, for old
  devices that can only fetch files.
- Dragging tabs out of a window and between windows; remembering session
  windows across restarts.
- Checking TLS certificates for ports a script reaches through a tunnel.
- Remote port forwards (`ssh -R`), and port forwards opened without a
  terminal, as a Files connection is.
- Quick tools: ping, traceroute, a subnet calculator, MAC vendor lookup.
- A topology map from LLDP and CDP neighbours.
- **Waiting until the app is further along:**
  - A macOS release
    ([#171](https://github.com/ssosa-dev-net/enodix/issues/171)).
  - Code signing for Windows
    ([#169](https://github.com/ssosa-dev-net/enodix/issues/169)). The
    installer is not code-signed (Authenticode), so SmartScreen warns the
    first time and some managed Windows computers refuse it. Updates are
    signed for the app's own updater already.
  - A phone or tablet app
    ([#173](https://github.com/ssosa-dev-net/enodix/issues/173)).

## Not planned

- Scanning subnets or discovering unknown devices.
- A manual diagram editor.
- Telemetry, crash reporting to third parties, AI features.
- Hosted sync, accounts or any cloud service.
- Embedded web consoles or stored web logins.
- An embedded Python interpreter.
- Encrypting session logs, job results or config backups: left to the
  disk's own encryption
  ([#154](https://github.com/ssosa-dev-net/enodix/issues/154)).
- A NETCONF and RESTCONF client: scripts reach those ports through tunnels
  with the Python libraries engineers already use.

## Known limits

- A host key that changed is noticed when a job connects, not at pre-flight.
- Through a script's tunnel the app checks no TLS certificate; the script
  must (eAPI, RESTCONF, NX-API).
- ncclient cannot negotiate with a device for which the app trusts only an
  RSA key that it offers as `rsa-sha2-*` only (an ncclient limit).
- The Nornir plugin checks a directly reached host's key only when the app
  already trusts one for it; it warns, naming the others.
- A main window that hangs is not noticed (session windows are).
