//! File connections: a login to a device, through its jump hosts, that
//! opens no terminal (see [`crate::files`]). It logs in exactly as a
//! terminal session does (the target, the jump chain, host-key checks, the
//! login flow, legacy crypto, connect timeouts, keepalives) and stops before
//! the shell.

use std::sync::Arc;

use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};

pub(crate) use super::ssh::SubsystemRefusal;
use super::ssh::{self, Connected};
use super::{
    HostKeyAsker, HostKeyDecision, HostKeyQuestion, LoginPrompt, NegotiatedAlgorithms, OpenError,
    Prompter, SessionEvent, SshTarget, UserWait,
};
use crate::known_hosts::KnownHosts;

/// A logged-in connection to a device that carries no terminal: one
/// channel at a time (SFTP, say: [`FileConnection::start_sftp`]).
///
/// It owns its connection and its jump connections: dropping it (or
/// [`FileConnection::close`]) closes the device's connection, then each
/// jump host's, the nearest first, whatever channel is still open on it.
/// Its SSH window is russh's (2 MiB), which downloads need.
pub struct FileConnection {
    connected: Connected,
    /// What it was opened with, to connect again (SCP connects for each
    /// transfer).
    target: SshTarget,
    known_hosts: KnownHosts,
    /// The one channel it may carry at a time.
    channel: Arc<Semaphore>,
}

/// Why an open file connection ended ([`FileConnection::lost`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Lost {
    /// The device ended it, saying `message` (often empty).
    #[error("the device closed the connection{}", parenthesised(.message))]
    Closed { message: String },
    /// The device stopped answering: no reply to the keepalives for about
    /// three connect timeouts.
    #[error("the device stopped answering")]
    NotAnswering,
    /// The connection broke for this reason (the network, the transport).
    #[error("the connection broke ({0})")]
    Broken(String),
    /// The connection to this jump host ended, and the device's with it.
    #[error("the connection to the jump host {name} ended")]
    JumpHost { name: String },
}

fn parenthesised(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        String::new()
    } else {
        format!(" ({message})")
    }
}

/// What a file connection being opened reports ([`Opening::next`]).
#[derive(Debug)]
pub enum OpeningEvent {
    /// A host key that is not trusted for its host: decide with
    /// [`Opening::decide_host_key`]. Nothing goes on until then.
    HostKey(HostKeyQuestion),
    /// A login question (a one-time code, a jump host's password): answer
    /// with [`Opening::answer`]. Dropping the opening refuses it.
    Prompt(LoginPrompt),
    /// Logged in. The last event.
    Opened(FileConnection),
    /// It could not be opened. The last event.
    Failed(OpenError),
}

/// A file connection being opened, attended: its questions reach the
/// caller, who answers them as a terminal session's owner does. Dropping it
/// gives up (the connection, if any, is closed).
pub struct Opening {
    events: mpsc::UnboundedReceiver<OpeningEvent>,
    answers: Option<mpsc::UnboundedSender<Vec<String>>>,
    decisions: mpsc::UnboundedSender<HostKeyDecision>,
    task: tokio::task::JoinHandle<()>,
}

/// Why a file connection opened unattended
/// ([`FileConnection::open_unattended`]) did not open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// The connection failed (the device or a jump host could not be
    /// reached, refused the login, ...).
    #[error("{0}")]
    Failed(OpenError),
    /// A host key was not trusted, and nobody was there to decide: the
    /// reason names the key.
    #[error("{}", crate::job::host_key_refused(.0))]
    HostKeyRefused(HostKeyQuestion),
    /// The login asked a question nobody was there to answer: the reason
    /// names it.
    #[error("{}", crate::job::login_question_refused(.0))]
    QuestionRefused(LoginPrompt),
}

impl FileConnection {
    /// Starts opening a file connection to `target`, checking host keys
    /// against `known_hosts` (a key trusted and remembered there is
    /// trusted on later connections; one trusted once, only this time).
    /// Returns at once; follow it with [`Opening::next`].
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open(target: SshTarget, known_hosts: KnownHosts) -> Opening {
        let (events_tx, events) = mpsc::unbounded_channel();
        let (asked_tx, mut asked) = mpsc::unbounded_channel();
        let (answers_tx, answers) = mpsc::unbounded_channel();
        let (decisions_tx, decisions) = mpsc::unbounded_channel();
        let user = UserWait::default();
        let mut prompter = Prompter {
            events: asked_tx.clone(),
            answers,
            jump_host: None,
            user: user.clone(),
        };
        let asker = HostKeyAsker {
            events: asked_tx,
            decisions,
            user,
        };
        let task = tokio::spawn(async move {
            let opened = {
                let opening =
                    ssh::open_connection(&target, known_hosts.clone(), asker, &mut prompter);
                tokio::pin!(opening);
                loop {
                    tokio::select! {
                        biased;
                        Some(event) = asked.recv() => {
                            let event = match event {
                                SessionEvent::HostKey(question) => OpeningEvent::HostKey(question),
                                SessionEvent::Prompt(prompt) => OpeningEvent::Prompt(prompt),
                                _ => continue,
                            };
                            let _ = events_tx.send(event);
                        }
                        opened = &mut opening => break opened,
                    }
                }
            };
            let _ = events_tx.send(match opened {
                Ok(connected) => OpeningEvent::Opened(FileConnection {
                    connected,
                    target,
                    known_hosts,
                    channel: Arc::new(Semaphore::new(1)),
                }),
                Err(error) => OpeningEvent::Failed(error),
            });
        });
        Opening {
            events,
            answers: Some(answers_tx),
            decisions: decisions_tx,
            task,
        }
    }

    /// Opens a file connection to `target` with nobody to ask: a host key
    /// not trusted in `known_hosts` (unknown or changed) is refused, and so
    /// is any login question, each with a reason naming it, as jobs refuse
    /// them.
    pub async fn open_unattended(
        target: SshTarget,
        known_hosts: KnownHosts,
    ) -> Result<Self, ConnectError> {
        let mut opening = Self::open(target, known_hosts);
        let mut refused = None;
        while let Some(event) = opening.next().await {
            match event {
                OpeningEvent::HostKey(question) => {
                    opening.decide_host_key(HostKeyDecision::Cancel);
                    refused.get_or_insert(ConnectError::HostKeyRefused(question));
                }
                OpeningEvent::Prompt(prompt) => {
                    opening.refuse_questions();
                    refused.get_or_insert(ConnectError::QuestionRefused(prompt));
                }
                OpeningEvent::Opened(connection) => return Ok(connection),
                OpeningEvent::Failed(error) => {
                    return Err(refused.unwrap_or(ConnectError::Failed(error)))
                }
            }
        }
        Err(refused.unwrap_or(ConnectError::Failed(OpenError::Protocol(
            "the connection ended while it was being opened".into(),
        ))))
    }

    /// What each connection agreed on, jump hosts first, the device last;
    /// [`NegotiatedAlgorithms::legacy`] names the outdated ones.
    pub fn algorithms(&self) -> &[NegotiatedAlgorithms] {
        &self.connected.algorithms
    }

    /// Whether the connection is still up (not lost, not closed).
    pub fn is_open(&self) -> bool {
        self.connected.ended.borrow().is_none() && self.connected.is_open()
    }

    /// Resolves once the connection has ended, with why: at once if it
    /// already has. Closing it from this side counts too.
    pub async fn lost(&self) -> Lost {
        let mut ended = self.connected.ended.clone();
        // An error means the connection is gone without a word of why.
        let _ = ended.wait_for(Option::is_some).await;
        let reason = ended.borrow().clone();
        if let Some(name) = self.connected.lost_jump_host() {
            return Lost::JumpHost { name: name.into() };
        }
        reason.unwrap_or_else(|| Lost::Broken("the connection ended".into()))
    }

    /// Closes the connection, then each jump host's, the nearest first,
    /// and waits until each has been told.
    pub async fn close(mut self) {
        if let Some(closing) = self.connected.closing() {
            closing.await;
        }
    }

    /// The channel it may carry, unless one is open already.
    pub(crate) fn channel_permit(&self) -> Option<OwnedSemaphorePermit> {
        self.channel.clone().try_acquire_owned().ok()
    }

    /// A channel running the subsystem `name`, once the device accepts it.
    pub(crate) async fn subsystem(
        &self,
        name: &str,
    ) -> Result<russh::Channel<russh::client::Msg>, SubsystemRefusal> {
        self.connected.subsystem(name).await
    }

    /// A channel running `command`.
    pub(crate) async fn exec(
        &self,
        command: &str,
    ) -> Result<russh::Channel<russh::client::Msg>, String> {
        self.connected.exec(command).await
    }

    /// The target and known hosts it was opened with.
    pub(crate) fn opened_with(&self) -> (&SshTarget, &KnownHosts) {
        (&self.target, &self.known_hosts)
    }

    /// Follows why the connection ended (`Some` once it has; the sender
    /// gone, too).
    pub(crate) fn ended(&self) -> watch::Receiver<Option<Lost>> {
        self.connected.ended.clone()
    }

    /// The device's connect timeout, from which a stalled request's limit
    /// is counted.
    pub(crate) fn connect_timeout(&self) -> std::time::Duration {
        self.connected.connect_timeout
    }
}

impl Drop for FileConnection {
    fn drop(&mut self) {
        let Some(closing) = self.connected.closing() else {
            return;
        };
        // Without a runtime, dropping the handles ends the connections too,
        // in no particular order.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(closing);
        }
    }
}

impl std::fmt::Debug for FileConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileConnection")
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl Opening {
    /// The next question, or how the opening ended ([`OpeningEvent::Opened`]
    /// or [`OpeningEvent::Failed`]); `None` after that.
    pub async fn next(&mut self) -> Option<OpeningEvent> {
        self.events.recv().await
    }

    /// Answers the last [`OpeningEvent::Prompt`]'s questions, in their
    /// order. Missing answers count as empty; extra ones are dropped.
    pub fn answer(&self, answers: Vec<String>) {
        if let Some(sender) = &self.answers {
            let _ = sender.send(answers);
        }
    }

    /// Decides the last [`OpeningEvent::HostKey`] question.
    pub fn decide_host_key(&self, decision: HostKeyDecision) {
        let _ = self.decisions.send(decision);
    }

    /// Refuses the question asked, and any later: the login fails.
    fn refuse_questions(&mut self) {
        self.answers = None;
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        self.task.abort();
    }
}
