# Files on a device

Moving a file to or from a device (a software image onto flash, a config
or a tech-support bundle off it) without a second tool: the app already
has the device's address, login, jump hosts and legacy-crypto setting, and
knows which hosts are production.

There are two ways in: the **Files tab** of one host, and **Send file…**,
a job that sends one file to many hosts. Both work over SSH only (not
telnet, not serial, not a quick connection).

## The Files tab

**Files** on a host's details sheet, in the tree's context menu and in a
session's menu (or its tab's menu) opens a tab named "Files" and the
host's name, beside the terminals, in the main window. Asked from a
session window, it opens in the main window.

The tab makes a connection of its own, the way a terminal would: the
host's credential profile, its jump hosts, its legacy-crypto setting and
timeouts. Nothing is asked when the profile is stored and the keys are
trusted. A host with no stored profile asks for the username and password
in the tab; an unknown or changed host key is asked there too, with the
same choices as in a terminal. An open terminal on the same device is not
touched: on the lab, a terminal on cEOS echoed and answered as quickly
during a 400 MB upload as before it.

### A device with SFTP

The tab shows the device's files: folders first, then files by name,
hidden files included, each with its size, time and permissions.

- It starts in the folder where files belong on the host's platform (see
  [Where files belong](#where-files-belong)), or else where the login
  lands.
- The path box takes an absolute path, or one from the folder shown. A
  folder's name opens it; there are Up, Refresh and New folder, and Rename
  and Delete on each row (a file or an empty folder).
- **Upload** opens a file dialog (several files), or drop files from
  Explorer on the tab. They go into the folder shown.
- Tick files and **Download** puts them in a folder you pick; the next
  download starts there.
- The tab stays connected while it is open. A lost connection is said with
  its reason and a Reconnect button; transfers that were waiting run once
  it is back.

### A device with SCP only

Some devices have no SFTP: IOS and IOS-XE have none, and a Junos device
has none until its SFTP server is turned on. The app tries SFTP first and
falls back to SCP by itself. SCP can send one file and fetch one file; it
cannot list a folder, rename or delete. So the tab shows a form in place
of the listing, and says why:

- **Send a file**: a local file (chosen, or dropped on the tab) and the
  path on the device. The path starts as the platform's file folder
  (`flash:`) and gets the file's name once a file is chosen
  (`flash:image.bin`), until you type over it.
- **Fetch a file**: the path on the device, typed, and the local folder.

With SCP the app connects for each file and holds no connection in
between: IOS-XE closes the connection after each transfer anyway, and an
idle connection would take one of the device's few lines. The tab says
that no connection is held.

So each file is a new login. A stored profile makes that invisible, and a
login typed when the tab opened is kept while the tab is open and used for
each file. What the device itself asks at each login is asked over the
tab, as it was when the tab opened: a one-time code, or a host key that
was trusted once and not remembered. The transfer shows as connecting
until you answer, however long that takes. Cancel fails that transfer
alone ("the login was cancelled", or the refused key); the next one
connects and asks for itself. Closing the tab with a question open
cancels the transfer.

### What a device needs

| Platform | What it offers | What must be set on the device | Account |
| -------- | -------------- | ------------------------------ | ------- |
| Cisco IOS, IOS-XE | SCP only | `ip scp server enable`. Before it the device answers `Administratively disabled.`, which the app shows with that command as a hint. `ip ssh bulk-mode` makes uploads several times faster | Privilege 15; a lower one gets `Privilege denied.` |
| Juniper Junos | SCP as it comes; SFTP (the browser) once turned on | `set system services ssh sftp-server` for the browser. Without it the tab shows the SCP form, with that command as a hint | Any login class; a read-only login cannot delete |
| Arista EOS | SFTP | Nothing | Privilege 15. A lower one looks like a device without SFTP and then refuses SCP too, in its own words |
| Nokia SR Linux | SFTP | Nothing | The `admin` account was the one tried |
| FRR (Linux) | SFTP | An SSH server with its SFTP subsystem | Any |

A refusal is always shown as the device said it. The app adds a hint only
in the two cases above.

### Transfers

Below the listing (or the form) is the list of transfers: each one's
direction, name, progress and speed.

- One transfer runs at a time per host; the rest wait in order and can be
  removed. A running one can be cancelled.
- Transfers go on while another tab is shown. Closing the tab, closing the
  main window or switching workspace while one runs or waits asks first.
- After a transfer the size is checked: a download against the size the
  device announced, an SFTP upload against what the device then holds. An
  SCP upload cannot be checked this way, and the transfer says so.
- An upload shows the MD5 and SHA-256 of the local file, each with a copy
  button, to compare with the device's own check: `verify /md5 flash:x`
  on IOS, `bash md5sum /mnt/flash/x` on EOS, `file checksum md5 /var/tmp/x`
  on Junos. The app does not run that command for you.
- A cancelled or failed SFTP upload removes its partial file and says so;
  where it cannot (the connection is gone, the login may not delete) it
  says the partial file remains. Over SCP nothing can be removed: the
  transfer says a partial file may remain.
- A transfer in which no byte moves for three connect timeouts (45 s with
  the default timeout of 15 s) fails with that reason.
- Finished transfers stay listed while the tab is open.

### Production hosts and existing files

- A production host's tab has the production colour and mark. Uploading,
  deleting, renaming and making a folder there ask first, with Cancel
  focused. Downloading never asks.
- On any host, an upload over a file of the same name asks first, naming
  the files; a download asks before replacing a local file.
- Over SCP the device cannot be asked whether a file is there. The form
  says that a file of that name is replaced.

## Send a file to many hosts

**Send file…** is beside Run snippet and Run script: on a folder's or
host's details sheet, in the tree's context menu and on a saved filter.
Like every job it takes a folder, a saved filter or one host; to send to
a few chosen hosts, save a filter that shows them. It opens a job tab:

1. The file (Browse…, or a typed path), a destination folder and
   **Overwrite**. An empty destination means each host's platform file
   folder, so one run can stage an image on mixed platforms. A typed one
   may use host variables, such as `/images/{{site}}`.
2. The list beforehand: the file's name and size, where it goes on each
   host, and the hosts that will be skipped with the reason (not SSH, no
   stored profile, a jump host that needs a typed login, no destination).
   Production hosts are confirmed once, as for every job.
3. The run: three hosts at once unless changed. Each host shows Waiting,
   Sending with its progress, Sent, Already there or Failed with the
   reason. Nothing is asked: a host with an untrusted key or a login
   question fails with that reason and the others go on. Cancel stops it.

A host that already has the file (same name, same size) counts as done,
so running the job again sends only what is missing. A file of that name
with another size fails the host, and so does a host with SCP only, which
cannot say whether the file is there, unless **Overwrite** is ticked.

The kept run records the file's name, size, MD5 and SHA-256, the
destination and what happened on each host. A file job cannot be saved as
a job (it names a file on this computer); "Run again" works.

## Where files belong

Each platform definition can name the folder where files belong on that
platform (Platforms tab, "File folder"). The Files tab starts there, the
SCP form fills it in and the file job sends there.

| Built-in platform | File folder | Checked on |
| ----------------- | ----------- | ---------- |
| `ios` | `flash:` | CSR 1000v, IOS-XE 17.03.08 (where `bootflash:` is the same storage: a file sent to one is fetched from the other) |
| `eos` | `/mnt/flash` | cEOS 4.36.0.1F |
| `junos` | `/var/tmp` | cJunos Evolved 25.4R1.13 |
| `nxos` | `bootflash:` | not verified (no image in the lab) |
| `srlinux`, `frr` | none: the login's folder | SR Linux 24.10.1, FRR 10.2.1 |

## What was verified

What is listed here was checked against the containerlab lab
([containerlab.md](containerlab.md)), through its jump host, on 2026-11-21,
and against the app's own test servers. What was not is under
[Known limits](#known-limits). The lab is virtual machines and
containers on one server: its speeds say nothing about real hardware (the
lab's CSR 1000v is held to 1 Mbit/s by its licence).

- SFTP on SR Linux, cEOS, FRR and cJunos Evolved: list, upload, download
  and compare, rename, delete.
- SCP on IOS-XE: the fall back by itself, send and fetch, the refusal and
  its hint with `ip scp server enable` taken out, and that the tab holds
  none of the device's lines (`show users` lists only the terminal).
- The Junos fall back and its hint with the SFTP server taken out.
- A 400 MB file uploaded to cEOS in the tab: 4 s, about 100 MB/s, while a
  terminal on the same device echoed and answered in 45 to 63 ms, as
  before the upload.
- The same file sent as a job to cEOS, cJunos (about 8 s each over SFTP)
  and the CSR (about two and a half minutes over SCP, 2.7 MB/s), with
  each device's own checksum equal to the MD5 the run shows. Without
  Overwrite: cEOS counted as already there, the CSR failed saying it
  cannot tell, and a host whose key was not trusted failed with that
  reason while the others went on. Cancelled part way, the job stopped
  within a second.
- A cancelled SFTP upload on cJunos: its partial file was removed.
- Not on a device, but against the app's test server: a transfer that
  stops moving (failed after three connect timeouts, saying so), a lost
  connection and Reconnect, and the production questions. Also only there:
  a one-time code and a host key trusted once, asked for each file on a
  device with SCP only, since no lab device asks a question at login.

## Known limits

- **SCP-only devices have no listing**, no rename and no delete. The app
  runs no `dir` or other command on a device to make up for it.
- **Over SCP an existing file is replaced without a question**, since the
  device cannot be asked. For the same reason the file job fails such a
  host unless Overwrite is ticked.
- **Over SCP a wrong folder fails only at the end**: IOS takes the whole
  file and then answers `Write failed`.
- **Over SCP each file is a new login**, so a device whose login asks a
  question each time (a one-time code) asks it for every file in the tab.
  The file job asks nothing: it fails such a host, and one whose key is
  not trusted, with that reason.
- **An SCP upload's size cannot be checked**, and a cancelled one may
  leave a partial file (IOS removes it itself; EOS and Junos keep it).
  Cancelling an upload that was replacing a file leaves neither on IOS:
  the old file is gone once the new one starts.
- **A Junos read-only login cannot delete**, not even its own files, so a
  cancelled upload's partial file stays; the transfer says so.
- **An IOS device with all its lines in use refuses the connection**
  before the login begins. The app says the device closed the connection
  and that its lines may all be in use.
- **Not verified:** NX-OS and its `bootflash:`, the name of flash on
  physical Catalyst switches, speeds on real hardware, a jump host that
  needs a typed login, Linux. A drag from Explorer and the system's file
  dialog were played by the tests, not operated. The Junos read-only
  login and the IOS device with its lines in use (above) were seen while
  the feature was designed and are covered by tests, and were not tried
  again in the finished app.
- **Renaming onto an existing name fails** on every device tried, and is
  reported as such.
- **No resume**, no whole folders, no editing in place, no change of
  permissions, no free-space display, no fetching a file from many hosts.
  A Files tab lives in the main window only.
- On a very slow link (a few tens of KB/s with the default timeout) a
  transfer can be failed as stalled although bytes still move.

## Trust

- The app listens on nothing for this. No file server is started; every
  file moves over an SSH connection the app opens to the device.
- Every file connection is one you started: opening a Files tab, sending
  or fetching a file in it, or starting a file job. Nothing connects in
  the background, and a tab that is closed has no connection.
- A downloaded file is a plain file where you put it, as readable as any
  other file there. An image or a config is as sensitive on your disk as
  on the device.
- A file job's kept run holds the file's name, size and hashes and each
  host's outcome, as plain files beside the other job results. It does not
  hold the file.
- Host keys are checked as for a terminal. A job never trusts a key: a
  host whose key is not trusted yet fails until you connect to it once.
