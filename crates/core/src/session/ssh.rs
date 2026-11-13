//! The SSH transport: an interactive shell over `russh`.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::PublicKeyOrCertificate;
use russh::{Channel, ChannelMsg, ChannelWriteHalf, Disconnect};

use super::{CloseReason, Command, Link, OpenError, SessionState, SshTarget, TerminalSize};

/// Terminal type requested for the remote pty; xterm.js emulates xterm.
const TERM: &str = "xterm-256color";

/// How long to wait for the TCP connection and SSH handshake together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

struct Client;

impl client::Handler for Client {
    type Error = russh::Error;

    // Every host key is accepted for now. Trust-on-first-use and the
    // changed-key warning are a separate piece of work (host-key checking).
    async fn check_server_key(
        &mut self,
        _key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// An open shell: the connection, its channel, and anything the shell sent
/// before it was confirmed.
struct Shell {
    connection: Handle<Client>,
    channel: Channel<client::Msg>,
    early_output: Vec<u8>,
}

/// Runs one SSH session over `link` until it ends.
pub(super) async fn run(target: SshTarget, link: &mut Link) -> CloseReason {
    let requested_size = link.size;
    let opened = tokio::select! {
        opened = open_shell(&target, requested_size) => opened,
        () = link.close_requested() => return CloseReason::ClosedLocally,
    };
    let Shell {
        connection,
        channel,
        early_output,
    } = match opened {
        Ok(shell) => shell,
        Err(error) => return CloseReason::Failed(error),
    };

    let (mut incoming, outgoing) = channel.split();

    link.state(SessionState::Connected);
    if link.size != requested_size && resize(&outgoing, link.size).await.is_err() {
        return CloseReason::ConnectionLost;
    }
    if !early_output.is_empty() && !link.output(early_output) {
        return close(&connection).await;
    }

    // Input is sent one piece at a time while output keeps being read. A
    // piece larger than the remote window waits for the device to make room,
    // and the device may not make room until its own output (such as the echo
    // of that input) has been read.
    let mut unsent: VecDeque<Vec<u8>> = VecDeque::new();
    let mut sending: Option<Sending<'_>> = None;
    let mut exit_status = None;
    let flow = link.flow();
    loop {
        if sending.is_none() {
            if let Some(bytes) = unsent.pop_front() {
                sending = Some(Box::pin(outgoing.data_bytes(bytes)));
            }
        }
        tokio::select! {
            sent = finish(&mut sending) => {
                if sent.is_err() {
                    return CloseReason::ConnectionLost;
                }
            }
            command = link.command() => match command {
                Command::Write(bytes) => unsent.push_back(bytes),
                Command::Resize(size) => {
                    if resize(&outgoing, size).await.is_err() {
                        return CloseReason::ConnectionLost;
                    }
                }
                Command::Close => return close(&connection).await,
            },
            // Output is read only while the consumer is less than a window
            // behind (flow control); commands are taken regardless.
            message = async {
                flow.room_to_read().await;
                incoming.wait().await
            } => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if !link.output(data.to_vec()) {
                        return close(&connection).await;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(ChannelMsg::Close) => {
                    close(&connection).await;
                    return CloseReason::RemoteExited { exit_status };
                }
                Some(_) => {}
                // The channel ended without the remote closing it: the
                // connection is gone, unless the shell had already exited.
                None if exit_status.is_some() => return CloseReason::RemoteExited { exit_status },
                None => return CloseReason::ConnectionLost,
            },
        }
    }
}

/// A piece of input on its way to the device.
type Sending<'a> = Pin<Box<dyn Future<Output = Result<(), russh::Error>> + Send + 'a>>;

/// Resolves when the piece being sent, if any, has been sent; never if there
/// is none. Dropping this future does not abandon the piece.
async fn finish(sending: &mut Option<Sending<'_>>) -> Result<(), russh::Error> {
    match sending {
        Some(in_flight) => {
            let sent = in_flight.await;
            *sending = None;
            sent
        }
        None => std::future::pending().await,
    }
}

async fn open_shell(target: &SshTarget, size: TerminalSize) -> Result<Shell, OpenError> {
    let config = Arc::new(client::Config::default());
    let address = (target.host.as_str(), target.port);
    let mut connection =
        match tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, address, Client)).await
        {
            Ok(Ok(connection)) => connection,
            Ok(Err(russh::Error::IO(error))) => return Err(OpenError::Connect(error.to_string())),
            Ok(Err(error)) => return Err(protocol(error)),
            Err(_) => return Err(OpenError::Connect("timed out".into())),
        };

    let auth = connection
        .authenticate_password(target.username.as_str(), target.password.as_str())
        .await
        .map_err(protocol)?;
    if !auth.success() {
        return Err(OpenError::AuthenticationFailed);
    }

    let mut channel = connection.channel_open_session().await.map_err(protocol)?;
    channel
        .request_pty(false, TERM, size.cols, size.rows, 0, 0, &[])
        .await
        .map_err(protocol)?;
    channel.request_shell(true).await.map_err(protocol)?;

    // Wait for the server to confirm the shell, keeping anything it prints
    // in the meantime.
    let mut early_output = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => break,
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                early_output.extend_from_slice(&data);
            }
            Some(ChannelMsg::Failure) => {
                return Err(OpenError::Protocol(
                    "the server refused to start a shell".into(),
                ));
            }
            Some(_) => {}
            None => {
                return Err(OpenError::Protocol(
                    "the server closed the connection before starting a shell".into(),
                ));
            }
        }
    }

    Ok(Shell {
        connection,
        channel,
        early_output,
    })
}

fn protocol(error: russh::Error) -> OpenError {
    OpenError::Protocol(error.to_string())
}

async fn resize(
    channel: &ChannelWriteHalf<client::Msg>,
    size: TerminalSize,
) -> Result<(), russh::Error> {
    channel.window_change(size.cols, size.rows, 0, 0).await
}

/// Disconnects from this side.
async fn close(connection: &Handle<Client>) -> CloseReason {
    let _ = connection
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    CloseReason::ClosedLocally
}
