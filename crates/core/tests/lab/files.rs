//! The lab check of file transfers (#108): each device with what
//! `docs/containerlab.md` ("Files on the lab devices") says it offers,
//! through the lab's jump host, the way a Files tab and a file job reach
//! it: an unattended file connection of the host's profile target,
//! `start_files` (SFTP, or the fall back to SCP by itself) and the
//! transfer engine.
//!
//! - SFTP: list the platform's file folder (else the login's), upload a
//!   small file, download it and compare, rename it, delete it.
//! - SCP, on a device without SFTP: send the file to the platform's file
//!   folder, fetch it and compare. SCP cannot delete, so the file stays on
//!   the device, under one fixed name that each run replaces.
//!
//! Two lab devices need settings pasted after each deploy
//! (`lab/configs/csr1-files.cfg`, `lab/configs/cjunos1-files.set`). A
//! device without them is NOT CHECKED, with the device's words and the
//! app's hint as the reason; it is recognised by those hints, which the
//! core gives for exactly these two cases.
//!
//! The file is small: the lab's CSR 1000v is held to 1 Mbit/s and SR Linux
//! takes uploads at about 0.3 MB/s.

use std::fs;
use std::future::Future;
use std::time::Duration;

use enodix_core::files::{
    join_path, FileConnection, Files, Link, Outcome as Ended, Protocol, Scp, Sftp, SizeCheck,
    Transfer, TransferError, Transfers,
};
use tempfile::TempDir;

use super::{Device, LabRun, Outcome, Platform};

/// The file's name on the device.
pub const NAME: &str = "enodix-lab-check.bin";
/// Its name once renamed (SFTP).
pub const RENAMED: &str = "enodix-lab-check-renamed.bin";
/// Its size: more than the engine's 256 KiB buffer, so more than one step.
pub const SIZE: usize = 300 * 1024;

/// The host's name in the check's transfer engine.
const HOST: &str = "lab-device";

/// Why a device's check stopped.
enum Stop {
    /// The device lacks a setting the lab's files turn on.
    NotChecked(String),
    Failed(String),
}

impl From<String> for Stop {
    fn from(why: String) -> Self {
        Stop::Failed(why)
    }
}

/// What the lab's documentation says a platform offers; `None`: whatever
/// the device offers is checked.
fn documented(platform: Platform) -> Option<Protocol> {
    match platform {
        Platform::Srlinux | Platform::Ceos | Platform::Frr | Platform::Vtysh | Platform::Junos => {
            Some(Protocol::Sftp)
        }
        Platform::Ios => Some(Protocol::Scp),
        Platform::Nxos | Platform::Other => None,
    }
}

pub async fn check(run: &LabRun, device: &Device) -> Outcome {
    match transfer(run, device).await {
        Ok(note) => Outcome::Passed(note),
        Err(Stop::NotChecked(why)) => Outcome::NotApplicable(why),
        Err(Stop::Failed(why)) => Outcome::Failed(why),
    }
}

async fn transfer(run: &LabRun, device: &Device) -> Result<String, Stop> {
    // Nobody answers a host-key question here: trust the keys first, as a
    // terminal session would have.
    let manager = run.trusting_manager(device).await?;
    let host = device.host_id();
    let platform = run.workspace.host_platform(&host);
    let folder = run.workspace.host_file_folder(&host).map(str::to_owned);
    let wait = run.timeout();

    let connection = within(
        wait * 2,
        "the file connection",
        FileConnection::open_unattended(run.ssh_target(device)?, manager.known_hosts()),
    )
    .await?
    .map_err(|error| format!("the file connection did not open: {error}"))?;
    let files = within(wait, "starting SFTP", connection.start_files(platform))
        .await?
        .map_err(|error| format!("files did not start: {error}"))?;

    let local = TempDir::new().map_err(|error| error.to_string())?;
    let bytes = content(SIZE);
    let source = local.path().join(NAME);
    fs::write(&source, &bytes).map_err(|error| error.to_string())?;
    let back = local.path().join("back.bin");
    let round = Round {
        source: &source,
        back: &back,
        bytes: &bytes,
        wait,
    };

    match files {
        Files::Sftp { connection, sftp } => {
            if documented(device.platform) == Some(Protocol::Scp) {
                connection.close().await;
                return Err(Stop::Failed(
                    "SFTP started on a device documented as having none (docs/containerlab.md)"
                        .into(),
                ));
            }
            let checked = over_sftp(&sftp, folder, &round).await;
            drop(sftp);
            connection.close().await;
            Ok(checked?)
        }
        Files::Scp(scp) => {
            let why = scp.no_sftp();
            if documented(device.platform) == Some(Protocol::Sftp) {
                return Err(match scp.hint() {
                    Some(hint) => Stop::NotChecked(format!(
                        "no SFTP: {why}. The app's hint: {hint}. On the lab's cjunos1, paste \
                         lab/configs/cjunos1-files.set (docs/containerlab.md)"
                    )),
                    None => Stop::Failed(format!(
                        "no SFTP ({why}) on a device documented as offering it \
                         (docs/containerlab.md)"
                    )),
                });
            }
            // The CSR 1000v knows its flash by two names: sent to the
            // platform's `flash:`, the file is fetched from `bootflash:`.
            let fetch_folder = (device.platform == Platform::Ios
                && folder.as_deref() == Some("flash:"))
            .then(|| "bootflash:".to_owned())
            .or_else(|| folder.clone());
            let note = over_scp(scp, folder, fetch_folder, &round).await?;
            Ok(format!("no SFTP ({why}), fell back by itself; {note}"))
        }
    }
}

/// The local side of the check.
struct Round<'a> {
    /// The file sent.
    source: &'a std::path::Path,
    /// Where it comes back.
    back: &'a std::path::Path,
    bytes: &'a [u8],
    /// How long each answer may take.
    wait: Duration,
}

impl Round<'_> {
    /// Sends the file to `remote` through `transfers`; how that ended.
    async fn up(&self, transfers: &Transfers, remote: &str) -> Ended {
        let (_, watch) = transfers.add(HOST, Transfer::upload(self.source, remote));
        self.ended(watch.outcome()).await
    }

    /// Whether an upload that ended so sent the whole file, and whether
    /// its size on the device was checked.
    fn sent(&self, ended: &Ended) -> Option<SizeCheck> {
        match ended {
            Ended::Done(done) if done.bytes == self.bytes.len() as u64 => Some(done.size),
            _ => None,
        }
    }

    /// Fetches `remote` through `transfers` and compares it with what was
    /// sent.
    async fn down_and_compare(&self, transfers: &Transfers, remote: &str) -> Result<(), String> {
        let (_, watch) = transfers.add(HOST, Transfer::download(remote, self.back));
        match self.ended(watch.outcome()).await {
            Ended::Done(done) if done.size == SizeCheck::Checked => {}
            other => return Err(format!("the download of {remote} ended {}", ending(&other))),
        }
        let fetched = fs::read(self.back).map_err(|error| error.to_string())?;
        if fetched != self.bytes {
            return Err(format!(
                "{remote} came back different: {} bytes, {} sent",
                fetched.len(),
                self.bytes.len()
            ));
        }
        Ok(())
    }

    /// A transfer's end; one that does not end is cancelled by dropping
    /// the engine with the check.
    async fn ended(&self, outcome: impl Future<Output = Ended>) -> Ended {
        // A few answers each: the connection (SCP), the data, the close.
        let limit = self.wait * 4;
        match tokio::time::timeout(limit, outcome).await {
            Ok(ended) => ended,
            Err(_) => Ended::Failed {
                error: TransferError::Local(format!(
                    "the transfer did not end within {} s",
                    limit.as_secs()
                )),
                partial: enodix_core::files::Partial::Nothing,
            },
        }
    }
}

async fn over_sftp(
    sftp: &Sftp,
    folder: Option<String>,
    round: &Round<'_>,
) -> Result<String, String> {
    let wait = round.wait;
    let folder = match folder {
        Some(folder) => folder,
        None => within(wait, "the login folder", sftp.home())
            .await?
            .map_err(|error| format!("the login folder: {error}"))?,
    };
    let listed = listing(sftp, &folder, wait, "at the start").await?;
    let file = join_path(&folder, NAME);
    let renamed = join_path(&folder, RENAMED);
    // What a run that failed half way left.
    for (name, path) in [(NAME, &file), (RENAMED, &renamed)] {
        if listed.iter().any(|(entry, _)| entry == name) {
            within(wait, "clearing an old file", sftp.delete_file(path))
                .await?
                .map_err(|error| format!("an earlier run left {path}, and deleting it: {error}"))?;
        }
    }

    let transfers = Transfers::new();
    transfers.connect(HOST, Link::sftp(sftp.clone()));
    let uploaded = round.up(&transfers, &file).await;
    match round.sent(&uploaded) {
        Some(SizeCheck::Checked) => {}
        Some(unchecked) => return Err(format!("after the upload to {file}, {unchecked}")),
        None => return Err(format!("the upload to {file} ended {}", ending(&uploaded))),
    }
    let after = listing(sftp, &folder, wait, "after the upload").await?;
    if !after
        .iter()
        .any(|(name, size)| name == NAME && *size == Some(SIZE as u64))
    {
        return Err(format!(
            "{folder} does not list {NAME} with {SIZE} bytes after the upload"
        ));
    }
    round.down_and_compare(&transfers, &file).await?;
    transfers.close(HOST);

    within(wait, "the rename", sftp.rename(&file, &renamed))
        .await?
        .map_err(|error| format!("renaming {file}: {error}"))?;
    within(wait, "the delete", sftp.delete_file(&renamed))
        .await?
        .map_err(|error| format!("deleting {renamed}: {error}"))?;
    let left = listing(sftp, &folder, wait, "at the end").await?;
    if let Some((name, _)) = left
        .iter()
        .find(|(name, _)| name == NAME || name == RENAMED)
    {
        return Err(format!("{folder} still lists {name} after the delete"));
    }
    Ok(format!(
        "SFTP in {folder}: listed ({} entries), uploaded {} KiB (size checked), downloaded and \
         compared, renamed, deleted",
        listed.len(),
        SIZE / 1024
    ))
}

/// What `folder` lists `when` in the check: each entry's name and size.
async fn listing(
    sftp: &Sftp,
    folder: &str,
    wait: Duration,
    when: &str,
) -> Result<Vec<(String, Option<u64>)>, String> {
    let entries = within(wait, "the listing", sftp.list(folder))
        .await?
        .map_err(|error| format!("listing {folder} {when}: {error}"))?;
    Ok(entries
        .into_iter()
        .map(|entry| (entry.name, entry.size))
        .collect())
}

/// Sends the file into `folder` and fetches it from `fetch_folder` (the
/// same place, perhaps under its other name).
async fn over_scp(
    scp: Scp,
    folder: Option<String>,
    fetch_folder: Option<String>,
    round: &Round<'_>,
) -> Result<String, Stop> {
    // Without a file folder the name alone: the login's folder.
    let in_folder =
        |folder: Option<String>| folder.map_or_else(|| NAME.to_owned(), |f| join_path(&f, NAME));
    let file = in_folder(folder);
    let fetched = in_folder(fetch_folder);
    let transfers = Transfers::new();
    transfers.connect(HOST, Link::scp(scp));
    let sent = round.up(&transfers, &file).await;
    // The one refusal the core has a hint for: SCP turned off.
    let turned_off = match &sent {
        Ended::Failed {
            error: TransferError::Scp(error),
            ..
        } => error.hint().map(|hint| (error, hint)),
        _ => None,
    };
    match turned_off {
        _ if round.sent(&sent).is_some() => {}
        Some((error, hint)) => {
            return Err(Stop::NotChecked(format!(
                "{error} The app's hint: {hint}. On the lab's csr1, paste \
                 lab/configs/csr1-files.cfg (docs/containerlab.md)"
            )));
        }
        None => {
            return Err(Stop::Failed(format!(
                "the send to {file} ended {}",
                ending(&sent)
            )))
        }
    }
    round.down_and_compare(&transfers, &fetched).await?;
    transfers.close(HOST);
    let from = if fetched == file {
        String::new()
    } else {
        format!(" from {fetched}")
    };
    Ok(format!(
        "SCP: sent {} KiB to {file}, fetched{from} and compared (the file stays: SCP cannot \
         delete)",
        SIZE / 1024
    ))
}

/// How a transfer ended, for a failure's text.
fn ending(ended: &Ended) -> String {
    match ended {
        Ended::Done(done) => format!("done, with {} bytes; {}", done.bytes, done.size),
        Ended::Failed { error, partial } => format!("failed: {error}; {partial}"),
        Ended::Cancelled { partial } => format!("cancelled; {partial}"),
    }
}

/// `step` within `wait`, or what did not answer.
async fn within<T>(wait: Duration, what: &str, step: impl Future<Output = T>) -> Result<T, String> {
    tokio::time::timeout(wait, step)
        .await
        .map_err(|_| format!("{what} did not answer within {} s", wait.as_secs()))
}

/// Bytes that do not repeat in short runs.
fn content(size: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_u32;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}
