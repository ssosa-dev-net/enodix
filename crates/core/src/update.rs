//! The update check: is a newer release out?
//!
//! Releases publish a manifest, `latest.json`, in the Tauri updater's static
//! format (`version`, `notes`, `pub_date`, `platforms`), plus our own `page`
//! (the release's web page). [`UpdateChecker`] fetches it with one GET and
//! compares versions. What the user is offered depends on how this copy was
//! deployed ([`InstallKind`]):
//!
//! - an installed copy offers to install ([`UpdateAction::Install`]); the
//!   shell installs only when the user says so, through the Tauri updater,
//!   which checks the download's signature against the release key;
//! - every other copy (the portable zip, marked by [`PORTABLE_MARKER`]; a
//!   program copied out of an installation; a development build) only
//!   notifies ([`UpdateAction::Notify`]) and never replaces itself.
//!
//! The checker remembers the last offer: [`UpdateChecker::installable`]
//! decides whether the shell may install it.
//!
//! The check is the only connection the app makes on its own, and
//! [`UpdateChecker::automatic`] makes none at all when the user turned it off
//! (`Preferences::check_for_updates`). [`UpdateChecker::check_now`] is a check
//! the user asked for. Nothing about the user or the machine is sent: the
//! request is a plain GET of the manifest's address.

use std::net::IpAddr;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{Host, Url};

use crate::preferences::Preferences;
use crate::workspace::web_url;

/// The file that makes a copy portable: the release script puts it beside
/// the program in the portable zip.
pub const PORTABLE_MARKER: &str = "portable.txt";

/// The uninstaller the installer (Tauri's NSIS script) writes beside the
/// program: only a copy next to it counts as installed.
pub const INSTALLED_MARKER: &str = "uninstall.exe";

/// How long a check may take altogether.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// A manifest larger than this is refused (a real one is a few hundred bytes).
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// How this copy of the app was deployed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallKind {
    /// Set up by the installer: updates can be installed.
    Installed,
    /// Anything else (the portable zip first of all): updates are only
    /// announced, so that installing never adds a second copy.
    Portable,
}

impl InstallKind {
    /// Installed when the installer's uninstaller ([`INSTALLED_MARKER`]) is a
    /// file in `program_folder` (the folder holding the running program) and
    /// [`PORTABLE_MARKER`] is not; portable otherwise.
    pub fn detect(program_folder: &Path) -> Self {
        let has = |name: &str| program_folder.join(name).is_file();
        if has(INSTALLED_MARKER) && !has(PORTABLE_MARKER) {
            Self::Installed
        } else {
            Self::Portable
        }
    }
}

/// Why an address cannot be the release server's.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UpdateSourceError {
    #[error("this is not a web address")]
    NotAUrl,
    #[error("the release server must be reached over https, not {scheme}")]
    NotHttps { scheme: String },
}

/// The address of the release manifest: https, or plain http to this
/// computer only (tests, the smoke test's local server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSource {
    url: Url,
}

impl UpdateSource {
    pub fn new(text: &str) -> Result<Self, UpdateSourceError> {
        let url = Url::parse(text.trim()).map_err(|_| UpdateSourceError::NotAUrl)?;
        let allowed = match url.scheme() {
            "https" => true,
            "http" => is_this_computer(&url),
            _ => false,
        };
        if !allowed {
            return Err(UpdateSourceError::NotHttps {
                scheme: url.scheme().to_string(),
            });
        }
        Ok(Self { url })
    }

    /// The release server to ask: `override_address` when given (it wins,
    /// and a bad one means no check, never a fall-back to another server),
    /// else `built_in` (a release build's), else none.
    pub fn choose(override_address: Option<&str>, built_in: Option<&str>) -> Option<Self> {
        match override_address {
            Some(text) => Self::new(text).ok(),
            None => Self::new(built_in?).ok(),
        }
    }

    pub fn url(&self) -> &str {
        self.url.as_str()
    }
}

fn is_this_computer(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// The running version is not a version (`major.minor.patch`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{version} is not a version: {reason}")]
pub struct VersionError {
    pub version: String,
    pub reason: String,
}

/// What the user may do about a newer release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateAction {
    /// Install it (when the user agrees).
    Install,
    /// Only tell the user: they download it themselves.
    Notify,
}

/// A newer release than the running one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOffer {
    /// The release's version, without a leading `v`.
    pub version: String,
    /// The running version.
    pub current: String,
    pub notes: Option<String>,
    /// When it was published, as the manifest says (RFC 3339).
    pub published: Option<String>,
    /// The release's web page (http/https only; anything else is left out).
    pub page: Option<String>,
    pub action: UpdateAction,
}

impl UpdateOffer {
    /// Whether `version` (as another party spells it, `v` or not) is the
    /// version offered.
    pub fn is_version(&self, version: &str) -> bool {
        match (parse_version(version), parse_version(&self.version)) {
            (Ok(theirs), Ok(ours)) => theirs == ours,
            _ => false,
        }
    }
}

/// Why the shell may not install.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstallRefused {
    #[error("no newer version has been found; check for updates first")]
    NothingOffered,
    #[error("this copy only announces updates: download the new version yourself")]
    NotifyOnly,
    #[error("this build cannot install updates (it was not made by the release script)")]
    NoUpdater,
}

/// The outcome of a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateStatus {
    /// The automatic check is turned off: nothing was asked.
    Disabled,
    /// This build knows no release server (a development build).
    NotConfigured,
    UpToDate {
        current: String,
    },
    Available(UpdateOffer),
    /// The check did not work; the app goes on as before.
    Failed {
        reason: String,
    },
}

/// Checks the release server for a release newer than the running one, and
/// remembers what it offered.
#[derive(Debug)]
pub struct UpdateChecker {
    source: Option<UpdateSource>,
    current: semver::Version,
    kind: InstallKind,
    timeout: Duration,
    /// The newer release last found: replaced by each answer from the
    /// server, kept when a check fails.
    offer: Mutex<Option<UpdateOffer>>,
}

impl UpdateChecker {
    /// `source` is `None` for a build without a release server (then no
    /// check ever connects); `current_version` is the running version.
    pub fn new(
        source: Option<UpdateSource>,
        current_version: &str,
        kind: InstallKind,
    ) -> Result<Self, VersionError> {
        Ok(Self {
            source,
            current: parse_version(current_version).map_err(|reason| VersionError {
                version: current_version.to_string(),
                reason,
            })?,
            kind,
            timeout: DEFAULT_CHECK_TIMEOUT,
            offer: Mutex::new(None),
        })
    }

    /// The same checker, giving up after `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn kind(&self) -> InstallKind {
        self.kind
    }

    pub fn source(&self) -> Option<&UpdateSource> {
        self.source.as_ref()
    }

    /// The running version.
    pub fn current_version(&self) -> String {
        self.current.to_string()
    }

    /// The offer the shell may install once the user agreed:
    /// `updater_present` says whether the build has the Tauri updater.
    pub fn installable(&self, updater_present: bool) -> Result<UpdateOffer, InstallRefused> {
        let offer = self
            .offer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(InstallRefused::NothingOffered)?;
        if offer.action != UpdateAction::Install {
            return Err(InstallRefused::NotifyOnly);
        }
        if !updater_present {
            return Err(InstallRefused::NoUpdater);
        }
        Ok(offer)
    }

    /// The offer last made, if any (installable or not).
    pub fn offer(&self) -> Option<UpdateOffer> {
        self.offer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The check the app makes on its own (at start): none at all, and no
    /// connection, when the user turned it off.
    pub async fn automatic(&self, preferences: &Preferences) -> UpdateStatus {
        if !preferences.check_for_updates {
            return UpdateStatus::Disabled;
        }
        self.check_now().await
    }

    /// A check the user asked for, whatever the preference.
    pub async fn check_now(&self) -> UpdateStatus {
        let Some(source) = &self.source else {
            return UpdateStatus::NotConfigured;
        };
        let status = match self
            .fetch(source)
            .await
            .and_then(|body| self.status_from_manifest(&body))
        {
            Ok(status) => status,
            Err(reason) => return UpdateStatus::Failed { reason },
        };
        *self.offer.lock().unwrap_or_else(PoisonError::into_inner) = match &status {
            UpdateStatus::Available(offer) => Some(offer.clone()),
            _ => None,
        };
        status
    }

    async fn fetch(&self, source: &UpdateSource) -> Result<Vec<u8>, String> {
        install_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .connect_timeout(self.timeout)
            .build()
            .map_err(|error| format!("the update check could not start: {error}"))?;
        let unreachable =
            |error: reqwest::Error| format!("the release server could not be reached: {error}");
        let mut response = client
            .get(source.url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(unreachable)?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("the release server answered {status}"));
        }
        let too_big = || format!("the release manifest is larger than {MAX_MANIFEST_BYTES} bytes");
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
        {
            return Err(too_big());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            if body.len() + chunk.len() > MAX_MANIFEST_BYTES {
                return Err(too_big());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    fn status_from_manifest(&self, body: &[u8]) -> Result<UpdateStatus, String> {
        let manifest: Manifest = serde_json::from_slice(body)
            .map_err(|error| format!("the release manifest is not readable: {error}"))?;
        let latest = parse_version(&manifest.version).map_err(|reason| {
            format!(
                "the release manifest's version {} is not a version: {reason}",
                manifest.version
            )
        })?;
        if latest <= self.current {
            return Ok(UpdateStatus::UpToDate {
                current: self.current.to_string(),
            });
        }
        Ok(UpdateStatus::Available(UpdateOffer {
            version: latest.to_string(),
            current: self.current.to_string(),
            notes: manifest.notes.filter(|notes| !notes.trim().is_empty()),
            published: manifest.pub_date,
            page: manifest.page.and_then(|page| web_url(&page).ok()),
            action: match self.kind {
                InstallKind::Installed => UpdateAction::Install,
                InstallKind::Portable => UpdateAction::Notify,
            },
        }))
    }
}

/// The parts of `latest.json` the check reads (the updater reads the rest).
#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    page: Option<String>,
}

/// A semantic version, with or without a leading `v`.
fn parse_version(text: &str) -> Result<semver::Version, String> {
    let text = text.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    semver::Version::parse(text).map_err(|error| error.to_string())
}

/// rustls needs a process-wide crypto provider; `ring`, as for SSH and the
/// Tauri updater (which installs the same one if it gets there first).
fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
