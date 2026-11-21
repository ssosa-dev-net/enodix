//! Running a file job (#107): one local file sent to each host through the
//! transfer engine, at most [`super::JobOptions::parallel`] hosts at once,
//! each over an unattended file connection of its own (SFTP, or SCP with
//! the engine's connection per transfer).
//!
//! Per host: wait for room, connect (an untrusted host key or a login
//! question fails the host with the reason, as every job), reach its files,
//! then, without overwrite, look whether the file is there: the same size
//! is [`HostState::AlreadyThere`], another size fails, and a device that
//! cannot tell (SCP) fails asking for overwrite. Then the file is sent,
//! its progress reported as [`JobEvent::Progress`]. Every host gets the
//! file pre-flight read (#137): its hashes are read once at the start, a
//! host whose file changed (size or modification time) since pre-flight is
//! sent nothing, and one whose sent bytes hash otherwise fails, what was
//! sent left on the device. Cancel stops the run:
//! hosts not started end cancelled, a running transfer is cancelled through
//! the engine, which deals with its partial file.

use std::sync::Arc;

use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::Instant;

use super::{
    HostResult, HostState, JobEvent, JobPlan, JobReport, PlannedFile, PlannedHost, SentFile,
    PROGRESS_EVERY,
};
use crate::files::{
    hash_file, Done, FileConnection, Files, Hashes, Link, Outcome, Partial, Presence, Status,
    Transfer, Transfers,
};
use crate::known_hosts::KnownHosts;
use crate::workspace::HostId;

/// Why a host fails when its device cannot say whether the file is there.
const CANNOT_TELL: &str =
    "this device cannot say whether the file is there; tick overwrite to send it anyway";

/// Runs the planned file job, reporting on `events`.
pub(super) fn spawn(
    plan: JobPlan,
    overwrite: bool,
    parallel: usize,
    known_hosts: KnownHosts,
    events: mpsc::UnboundedSender<JobEvent>,
    cancel: watch::Receiver<bool>,
) {
    let file = plan.file.clone().expect("a file job plans its file");
    for host in &plan.hosts {
        let _ = events.send(JobEvent::State {
            host: host.id.clone(),
            state: HostState::Waiting,
        });
    }
    // The planned file's hashes, read once at the start: what every host
    // must get (#137). Each host waits for them once its file is sent.
    let (planned, reference) = watch::channel(None);
    let hashed = file.clone();
    tokio::spawn(async move {
        let hashes = tokio::task::spawn_blocking(move || planned_hashes(&hashed))
            .await
            .unwrap_or_else(|error| Err(Reference::Unreadable(error.to_string())));
        planned.send_replace(Some(Arc::new(hashes)));
    });
    let room = Arc::new(Semaphore::new(parallel));
    let transfers = Transfers::new();
    let tasks: Vec<_> = plan
        .hosts
        .into_iter()
        .map(|host| {
            let named = (host.id.clone(), host.name.clone());
            let sender = Sender {
                id: host.id.clone(),
                events: events.clone(),
                transfers: transfers.clone(),
                known_hosts: known_hosts.clone(),
                file: file.clone(),
                reference: reference.clone(),
                overwrite,
                output: String::new(),
            };
            (
                named,
                tokio::spawn(sender.run(host, room.clone(), cancel.clone())),
            )
        })
        .collect();
    tokio::spawn(async move {
        let mut hosts = Vec::with_capacity(tasks.len());
        for ((id, name), task) in tasks {
            let result = task.await.unwrap_or_else(|error| {
                let state = HostState::Failed {
                    reason: format!("the job stopped on this host: {error}"),
                };
                let _ = events.send(JobEvent::State {
                    host: id.clone(),
                    state: state.clone(),
                });
                HostResult {
                    id,
                    name,
                    state,
                    output: String::new(),
                    backup: None,
                    exit_status: None,
                    facts: None,
                    bytes: None,
                }
            });
            hosts.push(result);
        }
        // A cancelled run ends now: hashing a large file is not waited for
        // (it goes on in the background and is dropped).
        let hashes = if *cancel.borrow() {
            None
        } else {
            match planned_reference(reference).await.as_deref() {
                Some(Ok(hashes)) => Some(hashes.clone()),
                _ => None,
            }
        };
        let file = SentFile {
            name: file.name,
            size: file.size,
            hashes,
        };
        let _ = events.send(JobEvent::Finished(JobReport {
            hosts,
            file: Some(file),
        }));
    });
}

/// Why the planned file's hashes are not known.
#[derive(Debug)]
enum Reference {
    /// It is no longer the file pre-flight read: why.
    Changed(String),
    /// It could not be read.
    Unreadable(String),
}

/// The planned file's hashes, if it is still the file pre-flight read
/// before and after they are read (blocking).
fn planned_hashes(file: &PlannedFile) -> Result<Hashes, Reference> {
    let unchanged = || match std::fs::metadata(&file.path) {
        Ok(metadata) => match differs(file, &metadata) {
            None => Ok(()),
            Some(how) => Err(Reference::Changed(how)),
        },
        Err(error) => Err(Reference::Unreadable(error.to_string())),
    };
    unchanged()?;
    let hashes = hash_file(&file.path).map_err(|error| Reference::Unreadable(error.to_string()))?;
    unchanged()?;
    Ok(hashes)
}

/// How the local file, as `metadata` describes it, differs from the one
/// pre-flight read (its size or its modification time); `None` if it
/// does not.
fn differs(file: &PlannedFile, metadata: &std::fs::Metadata) -> Option<String> {
    let size = metadata.len();
    if size != file.size {
        return Some(format!(
            "it was {} bytes at pre-flight and is {size} bytes now",
            file.size
        ));
    }
    let modified = metadata.modified().ok();
    if modified.is_some() && file.modified.is_some() && modified != file.modified {
        return Some("it was written to since pre-flight (its modification time changed)".into());
    }
    None
}

/// The words for a host's file not sent because the file changed.
fn changed_before(how: &str) -> String {
    format!(
        "the file changed since pre-flight ({how}): nothing was sent; plan the job again to send the file as it is now"
    )
}

/// The planned file's hashes, or why there are none, once read.
async fn planned_reference(
    mut reference: watch::Receiver<Option<Arc<Result<Hashes, Reference>>>>,
) -> Option<Arc<Result<Hashes, Reference>>> {
    reference
        .wait_for(Option::is_some)
        .await
        .ok()
        .and_then(|read| read.clone())
}

/// Resolves once the job is cancelled (never, if its handle is gone).
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Sends the file to one host.
struct Sender {
    id: HostId,
    events: mpsc::UnboundedSender<JobEvent>,
    transfers: Transfers,
    known_hosts: KnownHosts,
    file: PlannedFile,
    /// The planned file's hashes, once read at the start.
    reference: watch::Receiver<Option<Arc<Result<Hashes, Reference>>>>,
    overwrite: bool,
    /// What happened, for the user: the host's output.
    output: String,
}

/// A host connected and ready to send to.
struct Ready {
    link: Link,
    /// The SFTP connection, closed once the file is sent (SCP connects
    /// per transfer, in the engine).
    connection: Option<FileConnection>,
}

impl Sender {
    async fn run(
        mut self,
        host: PlannedHost,
        room: Arc<Semaphore>,
        mut cancel: watch::Receiver<bool>,
    ) -> HostResult {
        let permit = tokio::select! {
            permit = room.acquire_owned() => permit.ok(),
            () = cancelled(&mut cancel) => None,
        };
        let Some(permit) = permit else {
            return self.finish(host, HostState::Cancelled, None);
        };
        self.state(HostState::Connecting);
        let ready = tokio::select! {
            ready = self.connect(&host) => ready,
            () = cancelled(&mut cancel) => Err(HostState::Cancelled),
        };
        let (state, bytes) = match ready {
            Ok(ready) => self.send(&host, ready, &mut cancel).await,
            Err(state) => (state, None),
        };
        let result = self.finish(host, state, bytes);
        drop(permit);
        result
    }

    /// Connects, reaches the host's files and, without overwrite, looks
    /// whether the file is there; the host's final state if it ends here.
    async fn connect(&mut self, host: &PlannedHost) -> Result<Ready, HostState> {
        let remote = remote(host);
        let target = host
            .work
            .target
            .clone()
            .ok_or_else(|| failed("there is nothing to connect it with".into()))?;
        let connection = FileConnection::open_unattended(target, self.known_hosts.clone())
            .await
            .map_err(|error| failed(error.to_string()))?;
        let files = connection
            .start_files(host.work.definition.as_ref())
            .await
            .map_err(|error| failed(format!("its files cannot be reached: {error}")))?;
        let (link, connection) = match files {
            Files::Sftp { connection, sftp } => {
                self.line("Connected over SFTP.");
                (Link::sftp(sftp), Some(connection))
            }
            Files::Scp(scp) => {
                self.line(&format!(
                    "This device offers no SFTP ({}): sending over SCP.",
                    scp.no_sftp()
                ));
                (Link::scp(scp), None)
            }
        };
        let ready = Ready { link, connection };
        if self.overwrite {
            return Ok(ready);
        }
        let size = self.file.size;
        let ended = match ready.link.presence(remote).await {
            Ok(Presence::Absent) => None,
            Ok(Presence::Exists { size: Some(found) }) if found == size => {
                self.line(&format!(
                    "{remote} is already there with the same size ({size} bytes): nothing sent."
                ));
                Some(HostState::AlreadyThere)
            }
            Ok(Presence::Exists { size: Some(found) }) => Some(failed(format!(
                "a file of that name exists with another size ({found} bytes on the device, {size} to send); tick overwrite to replace it"
            ))),
            Ok(Presence::Exists { size: None }) => Some(failed(
                "a file of that name is there and the device does not say its size; tick overwrite to replace it".into(),
            )),
            Ok(Presence::CannotTell) => Some(failed(CANNOT_TELL.into())),
            Err(error) => Some(failed(format!(
                "whether the file is there could not be checked: {error}"
            ))),
        };
        match ended {
            None => Ok(ready),
            Some(state) => {
                if let Some(connection) = ready.connection {
                    connection.close().await;
                }
                Err(state)
            }
        }
    }

    /// Sends the file through the engine, reporting progress; cancelling
    /// cancels the transfer and waits for the engine to deal with its
    /// partial file. The final state and the bytes sent.
    async fn send(
        &mut self,
        host: &PlannedHost,
        ready: Ready,
        cancel: &mut watch::Receiver<bool>,
    ) -> (HostState, Option<u64>) {
        let remote = remote(host);
        // A file gone is the engine's to say (it names the file).
        if let Ok(metadata) = tokio::fs::metadata(&self.file.path).await {
            if let Some(how) = differs(&self.file, &metadata) {
                if let Some(connection) = ready.connection {
                    connection.close().await;
                }
                return (failed(changed_before(&how)), None);
            }
        }
        self.state(HostState::Running);
        self.line(&format!("Sending {} to {remote}.", self.file.name));
        let key = host.id.as_str();
        self.transfers.connect(key, ready.link);
        let (id, mut watch) = self
            .transfers
            .add(key, Transfer::upload(&self.file.path, remote));
        let mut sent_at: Option<Instant> = None;
        let mut unsent: Option<(u64, Option<u64>)> = None;
        let mut cancelling = false;
        let outcome = loop {
            let flush = sent_at.map_or_else(Instant::now, |at| at + PROGRESS_EVERY);
            tokio::select! {
                status = watch.changed() => match status {
                    Some(Status::Finished(outcome)) => break outcome,
                    None => match watch.status() {
                        Status::Finished(outcome) => break outcome,
                        _ => unreachable!("a watch ends finished"),
                    },
                    Some(Status::Running { done, total }) => {
                        if sent_at.is_some_and(|at| at.elapsed() < PROGRESS_EVERY) {
                            unsent = Some((done, total));
                        } else {
                            self.progress(done, total);
                            sent_at = Some(Instant::now());
                            unsent = None;
                        }
                    }
                    Some(Status::Waiting | Status::Connecting) => {}
                },
                () = tokio::time::sleep_until(flush), if unsent.is_some() => {
                    if let Some((done, total)) = unsent.take() {
                        self.progress(done, total);
                        sent_at = Some(Instant::now());
                    }
                }
                () = cancelled(cancel), if !cancelling => {
                    cancelling = true;
                    self.transfers.cancel(id);
                }
            }
        };
        self.transfers.close(key);
        if let Some(connection) = ready.connection {
            connection.close().await;
        }
        match outcome {
            Outcome::Done(done) => {
                self.line(&format!(
                    "Sent {} bytes to {remote}; {}.",
                    done.bytes, done.size
                ));
                match self.checked(&done, cancel).await {
                    None => (HostState::Done, Some(done.bytes)),
                    Some(reason) => (failed(reason), None),
                }
            }
            Outcome::Failed { error, partial } => {
                let reason = match partial {
                    Partial::Nothing => error.to_string(),
                    partial => format!("{error}; {partial}"),
                };
                (failed(reason), None)
            }
            Outcome::Cancelled { partial } => {
                self.line(&format!("Cancelled: {partial}."));
                (HostState::Cancelled, None)
            }
        }
    }

    /// Whether what was sent is the file pre-flight read, by its hashes:
    /// `None` if it is, or if that cannot be told (said in the output);
    /// else why the host fails. What was sent stays on the device.
    async fn checked(&mut self, done: &Done, cancel: &mut watch::Receiver<bool>) -> Option<String> {
        let reference = tokio::select! {
            reference = planned_reference(self.reference.clone()) => reference,
            () = cancelled(cancel) => {
                self.line("Not checked against the file pre-flight read: the job was cancelled first.");
                return None;
            }
        };
        match reference.as_deref() {
            Some(Ok(planned)) => match &done.hashes {
                Some(sent) if sent != planned => Some(format!(
                    "the file changed while it was sent: the device got another file than the one pre-flight read (SHA-256 {} sent, {} planned); what was sent was left on the device",
                    sent.sha256, planned.sha256
                )),
                _ => None,
            },
            Some(Err(Reference::Changed(how))) => Some(format!(
                "the file changed during the run ({how}), so what was sent may not be the file pre-flight read; it was left on the device"
            )),
            Some(Err(Reference::Unreadable(error))) => {
                self.line(&format!(
                    "Not checked against the file pre-flight read: it could not be read again ({error})."
                ));
                None
            }
            None => None,
        }
    }

    fn finish(mut self, host: PlannedHost, state: HostState, bytes: Option<u64>) -> HostResult {
        if let HostState::Failed { reason } = &state {
            self.line(&format!("Failed: {reason}"));
        }
        self.state(state.clone());
        HostResult {
            id: host.id,
            name: host.name,
            state,
            output: self.output,
            backup: None,
            exit_status: None,
            facts: None,
            bytes,
        }
    }

    /// One line of the host's output.
    fn line(&mut self, text: &str) {
        let text = format!("{text}\n");
        self.output.push_str(&text);
        let _ = self.events.send(JobEvent::Output {
            host: self.id.clone(),
            text,
        });
    }

    fn progress(&self, done: u64, total: Option<u64>) {
        let _ = self.events.send(JobEvent::Progress {
            host: self.id.clone(),
            done,
            total,
        });
    }

    fn state(&self, state: HostState) {
        let _ = self.events.send(JobEvent::State {
            host: self.id.clone(),
            state,
        });
    }
}

/// The path the file gets on `host`.
fn remote(host: &PlannedHost) -> &str {
    host.work
        .remote
        .as_deref()
        .expect("a file job plans each host's destination")
}

fn failed(reason: String) -> HostState {
    HostState::Failed { reason }
}
