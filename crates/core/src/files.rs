//! Files on a device: a logged-in connection without a terminal
//! ([`FileConnection`]) and SFTP on it ([`Sftp`]), which the Files tab and
//! file transfers stand on.
//!
//! A file connection logs in as a terminal session does and opens no shell.
//! Its questions (an untrusted host key, a login question) reach the caller
//! when it is opened attended ([`FileConnection::open`]) and are refused,
//! with a reason naming them, when it is not
//! ([`FileConnection::open_unattended`]). It carries one channel at a time.
//!
//! A device with no SFTP is reached over SCP ([`Scp`]), which only sends
//! and fetches one file, with a connection per transfer;
//! [`FileConnection::start_files`] falls back to it by itself.
//!
//! Transfers (whole files up and down, a waiting list per host, progress,
//! hashes, size checks, partial files, the stall rule) are the engine's,
//! [`Transfers`]; the Files tab and the file job only call it.
//!
//! SFTP's own failures are bare (`Failure: Failure`); [`FileError`] says
//! what happened in the app's words.

mod browser;
mod scp;
mod sftp;
mod transfer;

pub use crate::session::connection::{ConnectError, FileConnection, Lost, Opening, OpeningEvent};
pub use browser::{
    existing_locally, follow, join_path, parent_folder, resolve_path, Added, BrowseError,
    BrowserEvent, BrowserId, FileBrowsers, Listing, TransferState,
};
pub use scp::{Files, Protocol, Scp, ScpError};
pub use sftp::{
    EntryKind, FileError, NoSftp, RemoteEntry, RemoteReader, RemoteWriter, Sftp, StartError,
};
pub(crate) use transfer::hash_file;
pub use transfer::{
    Connect, Connecting, Direction, Done, Hashes, Link, Outcome, Partial, Presence, SizeCheck,
    Status, Transfer, TransferError, TransferId, TransferWatch, Transfers,
};
