//! Workspaces: an inventory stored as a folder of YAML files.
//!
//! Directories are the hierarchy. Each host is one YAML file (`*.yaml` or
//! `*.yml`), so that edits to different hosts never conflict in git. Each
//! directory may hold one settings file, [`FOLDER_SETTINGS_FILE`]. Other
//! files whose name starts with `_` are reserved for workspace-wide
//! definitions and are not hosts; hidden entries (a leading `.`, such as
//! `.git`) and files of other types are ignored.
//!
//! The files are the only source of truth. Loading never stops at a bad
//! file: each one becomes a [`Problem`] naming the file, and the rest of the
//! workspace loads.
//!
//! Edits (creating, renaming, moving and deleting folders and hosts, saving
//! a host's fields, a folder's description, tags and settings) go straight
//! to the files, and
//! the workspace reloads its tree afterwards. Files are written so that git
//! diffs show only real changes: keys the app does not change keep their
//! place and their exact value, new built-in keys take a fixed place
//! ([`HOST_KEY_ORDER`]), and saving a host with nothing changed leaves its
//! file untouched.
//!
//! The workspace keeps a search index over its hosts ([`Workspace::search`]),
//! rebuilt from the files every time it loads, and the saved filters of its
//! [`FILTERS_FILE`] ([`Workspace::filters`]).
//!
//! Changes made outside the app are picked up by watching the folder
//! ([`WorkspaceWatcher`]) and calling [`Workspace::refresh`].

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use crate::highlight::{HighlightRule, HighlightSource, Highlighter, ListedRule};
use crate::platform::{ListedPlatform, PlatformDefinition, PlatformName, PlatformSource};
use crate::session::{
    SerialParameters, SerialTarget, SshAuth, SshTarget, TelnetTarget, DEFAULT_SSH_PORT,
    DEFAULT_TELNET_PORT,
};
use crate::template::{MissingVariables, Variables};

mod filters;
mod highlights;
mod identity;
pub(crate) mod import;
mod jumps;
mod platforms;
mod roles;
mod search;
mod settings;
mod snippets;
mod watch;
mod web_links;

use crate::paste::PasteSafety;
pub use filters::{Condition, SavedFilter, FILTERS_FILE};
pub use highlights::HIGHLIGHTS_FILE;
pub use identity::WORKSPACE_FILE;
pub use jumps::JumpChainError;
pub use platforms::PLATFORMS_FILE;
pub use roles::{Role, RoleIcon, RoleSource, NEUTRAL_ROLE_COLOR, ROLES_FILE};
use search::SearchIndex;
pub use search::{FieldMatch, FilterView, SearchHit, SearchResults};
use settings::{
    apply_settings, settings_from_document, Level, BACKUP_ON_LOGIN, CONNECT_TIMEOUT,
    CREDENTIAL_PROFILE, ENVIRONMENT, JUMP_HOST, LEGACY_CRYPTO, LOGGING, PASTE_LINE_DELAY, PLATFORM,
    PORT, PROTOCOL, SERIAL, WEB_LINKS,
};
pub use settings::{
    Effective, EffectiveSerial, EffectiveSettings, Environment, FlowControl, Origin, Parity,
    Protocol, SerialSettings, Settings, WebLink, DEFAULT_BAUD_RATE,
    DEFAULT_CONNECT_TIMEOUT_SECONDS, DEFAULT_DATA_BITS, DEFAULT_STOP_BITS,
    MAX_CONNECT_TIMEOUT_SECONDS,
};
pub use snippets::{ExpandError, Snippet, SNIPPETS_FILE};
pub use watch::{Changes, WatchError, WatchOptions, WorkspaceWatcher};
pub use web_links::{web_url, HostWebLink, WebLinkError};

/// The settings file a directory may hold: its description, tags and
/// inherited settings.
pub const FOLDER_SETTINGS_FILE: &str = "_folder.yaml";

/// Host file keys.
const NAME: &str = "name";
const ADDRESS: &str = "address";
const VENDOR: &str = "vendor";
const MODEL: &str = "model";
const SERIAL_NUMBER: &str = "serial_number";
const SITE: &str = "site";
const BUILDING: &str = "building";
const ROOM: &str = "room";
const RACK: &str = "rack";
const RACK_UNIT: &str = "rack_unit";
const ROLE: &str = "role";
const DESCRIPTION: &str = "description";
const TAGS: &str = "tags";
const CUSTOM: &str = "custom";

/// Where the app puts a built-in key that a host file does not have yet:
/// right after the nearest key before it in this list that the file has.
/// Keys the app does not know stay where they are.
pub const HOST_KEY_ORDER: [&str; 27] = [
    NAME,
    ADDRESS,
    PROTOCOL,
    PORT,
    CREDENTIAL_PROFILE,
    JUMP_HOST,
    LEGACY_CRYPTO,
    CONNECT_TIMEOUT,
    LOGGING,
    BACKUP_ON_LOGIN,
    ENVIRONMENT,
    PASTE_LINE_DELAY,
    SERIAL,
    VENDOR,
    PLATFORM,
    MODEL,
    SERIAL_NUMBER,
    SITE,
    BUILDING,
    ROOM,
    RACK,
    RACK_UNIT,
    ROLE,
    DESCRIPTION,
    TAGS,
    WEB_LINKS,
    CUSTOM,
];

/// The same for a folder's settings file.
pub const FOLDER_KEY_ORDER: [&str; 15] = [
    DESCRIPTION,
    TAGS,
    PLATFORM,
    PROTOCOL,
    PORT,
    CREDENTIAL_PROFILE,
    JUMP_HOST,
    LEGACY_CRYPTO,
    CONNECT_TIMEOUT,
    LOGGING,
    BACKUP_ON_LOGIN,
    ENVIRONMENT,
    PASTE_LINE_DELAY,
    SERIAL,
    WEB_LINKS,
];

/// The extension of host files the app creates.
pub(crate) const HOST_EXTENSION: &str = "yaml";

/// A loaded workspace: its tree and the problems found while loading it.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    /// See [`Workspace::id`].
    id: Option<String>,
    tree: Folder,
    /// The built-in roles, then the workspace's own.
    roles: Vec<Role>,
    /// The saved filters, in file order.
    filters: Vec<SavedFilter>,
    /// The workspace's highlight rules, then the built-in ones.
    highlights: Vec<ListedRule>,
    /// `highlights` compiled.
    highlighter: Highlighter,
    /// The built-in platform definitions, then the workspace's own.
    platforms: Vec<ListedPlatform>,
    /// The snippets, in file order.
    snippets: Vec<Snippet>,
    index: SearchIndex,
    problems: Vec<Problem>,
    /// See [`Workspace::revision`].
    revision: u64,
}

/// A directory of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    /// The directory's name.
    pub name: String,
    /// The directory relative to the workspace root, `/`-separated; empty
    /// for the root.
    pub path: String,
    pub description: Option<String>,
    /// Tags every host below gets too.
    pub tags: Vec<String>,
    /// The settings this folder sets for everything below it.
    pub settings: Settings,
    /// Subfolders, ordered by name.
    pub folders: Vec<Folder>,
    /// Hosts in this directory, ordered by name.
    pub hosts: Vec<Host>,
}

impl Folder {
    /// The hosts in this folder and all folders below it.
    pub fn host_count(&self) -> usize {
        self.hosts.len() + self.folders.iter().map(Folder::host_count).sum::<usize>()
    }
}

/// Identifies a host within its workspace: the host file's path relative to
/// the workspace root, `/`-separated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(String);

impl HostId {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The folder path and the file name.
    fn split(&self) -> (&str, &str) {
        self.0.rsplit_once('/').unwrap_or(("", &self.0))
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A host, read from its own file.
///
/// The text fields are `None` when the file does not set them. To change a
/// host, change a copy and pass it to [`Workspace::write_host`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: HostId,
    /// `name` in the file; the file name without its extension when absent.
    pub name: String,
    /// The management address (`address` in the file).
    pub address: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub site: Option<String>,
    pub building: Option<String>,
    pub room: Option<String>,
    pub rack: Option<String>,
    /// Text, as units are sometimes written `U12` or `12-13`; written as a
    /// number when it is one.
    pub rack_unit: Option<String>,
    /// The id of the host's role (`role` in the file), as written; see
    /// [`Workspace::host_role`] for the role it names.
    pub role: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// The user's own fields (`custom` in the file), in file order.
    pub custom: Vec<CustomField>,
    /// The settings the host sets itself (`port`, `platform` and the
    /// rest); see
    /// [`Workspace::resolve_host`] for what applies to it.
    #[serde(default)]
    pub settings: Settings,
    /// Everything the file holds, in file order, so that keys this version
    /// does not know survive a rewrite.
    #[serde(skip)]
    document: Mapping,
}

/// One of a host's own key/value fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomField {
    pub key: String,
    pub value: String,
}

impl CustomField {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl Host {
    /// The optional built-in text fields with their file keys.
    fn text_fields(&self) -> [(&'static str, &Option<String>); 10] {
        [
            (VENDOR, &self.vendor),
            (MODEL, &self.model),
            (SERIAL_NUMBER, &self.serial_number),
            (SITE, &self.site),
            (BUILDING, &self.building),
            (ROOM, &self.room),
            (RACK, &self.rack),
            (RACK_UNIT, &self.rack_unit),
            (ROLE, &self.role),
            (DESCRIPTION, &self.description),
        ]
    }
}

/// A host with the settings that apply to it once inheritance is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedHost {
    pub host: Host,
    pub settings: EffectiveSettings,
}

impl ResolvedHost {
    /// The values snippets and web links fill their variables with (see
    /// [`crate::template`]): `name`, `address` and the other built-in fields
    /// by their file keys (`vendor`, `serial_number`, `rack_unit`, `role`
    /// (its id), ...), `platform` (the one that applies, perhaps a
    /// folder's), `tags` (inherited ones included, comma-separated),
    /// and each custom field as `custom.<key>` and, unless a built-in field
    /// has that name, `<key>`. Fields without a value (unset or blank) are
    /// left out, so that the user is asked for them.
    pub fn variables(&self) -> Variables {
        let host = &self.host;
        let mut values = Variables::new();
        let mut set = |name: &str, value: &str| {
            if !value.trim().is_empty() {
                values.set(name, value);
            }
        };
        set(NAME, &host.name);
        set(ADDRESS, &host.address);
        for (key, value) in host.text_fields() {
            set(key, value.as_deref().unwrap_or_default());
        }
        if let Some(platform) = &self.settings.platform {
            set(PLATFORM, &platform.value);
        }
        let tags: Vec<&str> = self
            .settings
            .tags
            .iter()
            .map(|tag| tag.value.as_str())
            .collect();
        set(TAGS, &tags.join(", "));
        let built_in = |key: &str| {
            [NAME, ADDRESS, PLATFORM, TAGS]
                .into_iter()
                .chain(host.text_fields().map(|(name, _)| name))
                .any(|name| name.eq_ignore_ascii_case(key))
        };
        for field in &host.custom {
            set(&format!("{CUSTOM}.{}", field.key), &field.value);
        }
        for field in host.custom.iter().filter(|field| !built_in(&field.key)) {
            set(&field.key, &field.value);
        }
        values
    }

    /// The SSH target for this host with a password login (as prompted
    /// for), on the resolved port, or SSH's default port when nothing sets
    /// one, with its legacy-crypto switch and connect timeout. The host alone: its jump hosts
    /// are left out (connect through [`crate::credentials::Profiles`],
    /// which adds them).
    pub fn ssh_target(&self, username: String, password: String) -> SshTarget {
        self.ssh_target_with(username, SshAuth::Password(password))
    }

    /// [`Self::ssh_target`] with any kind of login.
    pub fn ssh_target_with(&self, username: String, auth: SshAuth) -> SshTarget {
        SshTarget {
            host: self.host.address.clone(),
            port: self.port_or(DEFAULT_SSH_PORT),
            username,
            auth,
            jump_hosts: Vec::new(),
            legacy_crypto: self.settings.legacy_crypto.value,
            connect_timeout: self.connect_timeout(),
        }
    }

    /// The telnet target for this host: on the resolved port, or telnet's
    /// default when nothing sets one, with its connect timeout. Its jump
    /// hosts are not looked at (connect through
    /// [`crate::credentials::Profiles::telnet_target`], which refuses them).
    pub fn telnet_target(&self) -> TelnetTarget {
        TelnetTarget {
            host: self.host.address.clone(),
            port: self.port_or(DEFAULT_TELNET_PORT),
            connect_timeout: self.connect_timeout(),
        }
    }

    /// The serial target for this host: its resolved serial port and line
    /// parameters; `None` if nothing sets a serial port for it.
    pub fn serial_target(&self) -> Option<SerialTarget> {
        let serial = &self.settings.serial;
        Some(SerialTarget {
            port: serial.port.as_ref()?.value.clone(),
            parameters: SerialParameters {
                baud_rate: serial.baud_rate.value,
                data_bits: serial.data_bits.value,
                parity: serial.parity.value,
                stop_bits: serial.stop_bits.value,
                flow_control: serial.flow_control.value,
            },
        })
    }

    /// The resolved port, or `default` (the protocol's) if nothing sets one.
    pub(crate) fn port_or(&self, default: u16) -> u16 {
        self.settings
            .port
            .as_ref()
            .map_or(default, |port| port.value)
    }

    fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.settings.connect_timeout.value.into())
    }

    /// What applies to pastes into this host's sessions, and the
    /// environment they are marked with.
    pub fn paste_safety(&self) -> PasteSafety {
        PasteSafety::of(&self.settings)
    }
}

/// A file that could not be loaded, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The file (or directory) relative to the workspace root, `/`-separated.
    pub file: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

/// The workspace folder itself could not be opened.
#[derive(Debug, thiserror::Error)]
#[error("cannot open workspace {}: {reason}", path.display())]
pub struct OpenError {
    pub path: PathBuf,
    pub reason: String,
}

/// An edit that was refused or failed. A refused edit changes no file.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The folder or host is not in this workspace.
    #[error("{0} is not in this workspace")]
    NotFound(String),
    /// The target folder already has an entry with this name (ignoring
    /// case, and for hosts the file extension).
    #[error("{0} already exists")]
    AlreadyExists(String),
    /// The name cannot be a file or folder name on every system.
    #[error("\"{name}\" cannot be used as a name: {reason}")]
    InvalidName { name: String, reason: &'static str },
    /// The edit makes no sense (an empty address, a folder into itself, ...).
    #[error("{0}")]
    Invalid(String),
    /// Deleting the folder would delete hosts, and that was not confirmed.
    #[error("{folder} contains {hosts} {}", if *.hosts == 1 { "host" } else { "hosts" })]
    FolderHasHosts { folder: String, hosts: usize },
    /// The file system refused.
    #[error("cannot write {file}: {source}")]
    Io {
        file: String,
        #[source]
        source: io::Error,
    },
}

/// Whether deleting a folder may delete the hosts inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderDeletion {
    /// Refuse with [`EditError::FolderHasHosts`] if it holds any host.
    IfNoHosts,
    /// The user confirmed: delete it with everything in it.
    IncludingHosts,
}

impl Workspace {
    /// Loads the workspace folder at `root`. Only a missing or unreadable
    /// root fails; bad files inside it are reported by [`Self::problems`].
    pub fn open(root: impl AsRef<Path>) -> Result<Self, OpenError> {
        let root = root.as_ref().to_path_buf();
        let open_error = |reason: String| OpenError {
            path: root.clone(),
            reason,
        };
        let metadata = fs::metadata(&root).map_err(|error| open_error(error.to_string()))?;
        if !metadata.is_dir() {
            return Err(open_error("not a folder".into()));
        }
        fs::read_dir(&root).map_err(|error| open_error(error.to_string()))?;

        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let mut problems = Vec::new();
        let id = identity::load_id(&root, &mut problems);
        let tree = load_folder(&root, name, String::new(), &mut problems);
        let roles = load_roles(&root, &mut problems);
        let filters = load_filters(&root, &mut problems);
        let highlights = load_highlights(&root, &mut problems);
        let highlighter = compile_highlights(&highlights);
        let platforms = load_platforms(&root, &mut problems);
        let snippets = load_snippets(&root, &mut problems);
        let index = SearchIndex::build(&tree, |id| roles::find(&roles, id).map(|r| r.name.clone()));
        problems.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(Self {
            root,
            id,
            tree,
            roles,
            filters,
            highlights,
            highlighter,
            platforms,
            snippets,
            index,
            problems,
            revision: 0,
        })
    }

    /// Counts the reloads since the workspace was opened: every edit that
    /// wrote files and every [`Self::refresh`] that found a change. Of two
    /// views of the same workspace, the one with the higher revision is
    /// newer.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The folder this workspace was opened from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// What a credential store keeps this workspace's profiles under: its
    /// identifier ([`Self::id`]), so that they follow the workspace when its
    /// folder moves; until it has one, the folder's full path
    /// ([`Self::path_credential_scope`]).
    pub fn credential_scope(&self) -> String {
        match &self.id {
            Some(id) => id.clone(),
            None => self.path_credential_scope(),
        }
    }

    /// The root folder of the tree.
    pub fn tree(&self) -> &Folder {
        &self.tree
    }

    /// The roles hosts can have: the built-in ones, then the workspace's
    /// own (one with a built-in id replaces it in place).
    pub fn roles(&self) -> &[Role] {
        &self.roles
    }

    /// The role with this id (ignoring case), if the workspace has it.
    pub fn role(&self, id: &str) -> Option<&Role> {
        roles::find(&self.roles, id)
    }

    /// The role the host `id` names, if it has one: the workspace's role of
    /// that id, else a neutral one ([`RoleSource::Undefined`]), so a host
    /// with a role the workspace does not define still shows.
    pub fn host_role(&self, id: &HostId) -> Option<Role> {
        self.role_shown(self.host(id)?)
    }

    /// [`Self::host_role`] for every host that has a role, by host id.
    pub fn host_roles(&self) -> BTreeMap<HostId, Role> {
        fn walk(workspace: &Workspace, folder: &Folder, roles: &mut BTreeMap<HostId, Role>) {
            for host in &folder.hosts {
                if let Some(role) = workspace.role_shown(host) {
                    roles.insert(host.id.clone(), role);
                }
            }
            for child in &folder.folders {
                walk(workspace, child, roles);
            }
        }
        let mut roles = BTreeMap::new();
        walk(self, &self.tree, &mut roles);
        roles
    }

    /// [`ResolvedHost::paste_safety`] for every host, by host id: its
    /// environment (for its sessions' colours) and paste line delay.
    pub fn host_paste_safety(&self) -> BTreeMap<HostId, PasteSafety> {
        fn walk<'a>(
            folder: &'a Folder,
            levels: &mut Vec<Level<'a>>,
            found: &mut BTreeMap<HostId, PasteSafety>,
        ) {
            levels.push(Level::of(folder));
            for host in &folder.hosts {
                levels.push(Level {
                    origin: Origin::Host,
                    settings: &host.settings,
                    tags: &host.tags,
                });
                found.insert(host.id.clone(), PasteSafety::of(&settings::resolve(levels)));
                levels.pop();
            }
            for child in &folder.folders {
                walk(child, levels, found);
            }
            levels.pop();
        }
        let mut found = BTreeMap::new();
        walk(&self.tree, &mut Vec::new(), &mut found);
        found
    }

    fn role_shown(&self, host: &Host) -> Option<Role> {
        let role = host.role.as_deref()?;
        Some(
            self.role(role)
                .cloned()
                .unwrap_or_else(|| Role::undefined(role)),
        )
    }

    /// The hosts matching every word of `query`, best first, at most
    /// `limit` of them (see the search index's rules in `search.rs`). An
    /// empty query lists every host by name.
    pub fn search(&self, query: &str, limit: usize) -> SearchResults {
        self.index.search(query, limit)
    }

    /// The saved filters, in the order of the filters file.
    pub fn filters(&self) -> &[SavedFilter] {
        &self.filters
    }

    /// The hosts `filter` matches and the folders holding them.
    pub fn apply_filter(&self, filter: &SavedFilter) -> FilterView {
        self.index.apply(filter)
    }

    /// Saves `filter` to the filters file: in place of the filter of the
    /// same name (ignoring case), else after the others. Names and values
    /// are trimmed; a filter without a name, without conditions, with a
    /// blank value, an unknown field or a field other than `tag` given
    /// twice is refused.
    pub fn save_filter(&mut self, filter: &SavedFilter) -> Result<(), EditError> {
        let filter = filters::clean(filter).map_err(EditError::Invalid)?;
        self.edit_file(FILTERS_FILE.into(), |document| {
            filters::upsert(document, &filter);
            Ok(())
        })
    }

    /// Deletes the filter called `name` (ignoring case) from the filters
    /// file; the file goes when no filter is left.
    pub fn delete_filter(&mut self, name: &str) -> Result<(), EditError> {
        self.edit_file(FILTERS_FILE.into(), |document| {
            if filters::remove(document, name) {
                Ok(())
            } else {
                Err(EditError::NotFound(format!("filter {}", name.trim())))
            }
        })
    }

    /// The highlight rules: the workspace's own (file order), then the
    /// built-in ones (as the workspace changes them). Rules whose pattern
    /// cannot be used are listed with why; they highlight nothing.
    pub fn highlight_rules(&self) -> &[ListedRule] {
        &self.highlights
    }

    /// The highlight rules that apply, compiled: those turned on whose
    /// pattern can be used.
    pub fn highlighter(&self) -> &Highlighter {
        &self.highlighter
    }

    /// Saves `rule` to the highlights file, in place of the rule called
    /// `previous` (ignoring case), or as a new rule after the others when
    /// `previous` is `None`. A built-in rule keeps its name, and only what
    /// differs from the built-in one is written. A rule without a name, an
    /// unknown colour, a name another rule has, a new rule with a built-in
    /// name, or a rule turned on whose pattern cannot be used is refused (a
    /// rule with a broken pattern can be turned off).
    pub fn save_highlight_rule(
        &mut self,
        previous: Option<&str>,
        rule: &HighlightRule,
    ) -> Result<(), EditError> {
        let rule = highlights::clean(rule).map_err(EditError::Invalid)?;
        let replaced = match previous {
            Some(previous) => Some(self.highlight_rule(previous).ok_or_else(|| {
                EditError::NotFound(format!("highlight rule {}", previous.trim()))
            })?),
            None => None,
        };
        if let Some(old) = replaced {
            if old.source != HighlightSource::Workspace
                && !highlights::same(&old.rule.name, &rule.name)
            {
                return Err(EditError::Invalid(format!(
                    "the built-in highlight rule {} cannot be renamed",
                    old.rule.name
                )));
            }
        }
        let clash = self.highlights.iter().any(|listed| {
            highlights::same(&listed.rule.name, &rule.name)
                && replaced.is_none_or(|old| !highlights::same(&old.rule.name, &listed.rule.name))
        });
        if clash {
            return Err(EditError::AlreadyExists(format!(
                "highlight rule {}",
                rule.name
            )));
        }
        let previous = replaced.map_or(rule.name.clone(), |old| old.rule.name.clone());
        self.edit_file(HIGHLIGHTS_FILE.into(), |document| {
            highlights::write(document, &previous, &rule);
            Ok(())
        })
    }

    /// Deletes the workspace's highlight rule called `name` (ignoring
    /// case), or for a built-in rule the workspace changes, its changes. A
    /// built-in rule as built in cannot be deleted (it can be turned off).
    pub fn delete_highlight_rule(&mut self, name: &str) -> Result<(), EditError> {
        let listed = self
            .highlight_rule(name)
            .ok_or_else(|| EditError::NotFound(format!("highlight rule {}", name.trim())))?;
        if listed.source == HighlightSource::BuiltIn {
            return Err(EditError::Invalid(format!(
                "the highlight rule {} is built in: turn it off instead",
                listed.rule.name
            )));
        }
        let name = listed.rule.name.clone();
        self.edit_file(HIGHLIGHTS_FILE.into(), |document| {
            highlights::remove(document, &name);
            Ok(())
        })
    }

    fn highlight_rule(&self, name: &str) -> Option<&ListedRule> {
        self.highlights
            .iter()
            .find(|listed| highlights::same(&listed.rule.name, name))
    }

    /// The platform definitions: the built-in ones (as the workspace changes
    /// them), then the workspace's own (file order). Definitions with a
    /// pattern that cannot be used are listed with why; they are not used.
    pub fn platforms(&self) -> &[ListedPlatform] {
        &self.platforms
    }

    /// The definition a platform setting of `text` names: by name, else by
    /// other name, ignoring case (and surrounding blanks). `None` when no
    /// definition answers to it, or the one that does cannot be used.
    pub fn platform(&self, text: &str) -> Option<&PlatformDefinition> {
        crate::platform::find(&self.platforms, text)
    }

    /// The definition the platform that applies to host `id` (its own or
    /// inherited) names, if any (see [`Self::platform`]).
    pub fn host_platform(&self, id: &HostId) -> Option<&PlatformDefinition> {
        let resolved = self.resolve_host(id)?;
        self.platform(&resolved.settings.platform?.value)
    }

    /// Every name and other name of a definition, trimmed and in lower
    /// case, with the definition it finds (as [`Self::platform`] does, but
    /// also one that cannot be used), for the interface.
    pub fn platform_names(&self) -> BTreeMap<String, PlatformName> {
        self.platforms
            .iter()
            .flat_map(|listed| {
                std::iter::once(&listed.definition.name).chain(&listed.definition.other_names)
            })
            .filter_map(|text| {
                let found = crate::platform::find_listed(&self.platforms, text)?;
                let name = PlatformName {
                    definition: found.definition.name.clone(),
                    usable: found.error.is_none(),
                };
                Some((text.trim().to_lowercase(), name))
            })
            .collect()
    }

    /// Saves `definition` to the platforms file, in place of the definition
    /// called `previous` (ignoring case), or as a new one after the others
    /// when `previous` is `None`. A built-in definition keeps its name, and
    /// only what differs from the built-in one is written (nothing, when
    /// nothing does). Refused: no name or prompt, a pattern that cannot be
    /// used, a name or other name another definition has, a new definition
    /// with a built-in name.
    pub fn save_platform(
        &mut self,
        previous: Option<&str>,
        definition: &PlatformDefinition,
    ) -> Result<(), EditError> {
        let definition = platforms::clean(definition).map_err(EditError::Invalid)?;
        let replaced = match previous {
            Some(previous) => Some(
                self.listed_platform(previous)
                    .ok_or_else(|| EditError::NotFound(format!("platform {}", previous.trim())))?,
            ),
            None => None,
        };
        if let Some(old) = replaced {
            if old.source != PlatformSource::Workspace && !old.definition.is_named(&definition.name)
            {
                return Err(EditError::Invalid(format!(
                    "the built-in platform {} cannot be renamed",
                    old.definition.name
                )));
            }
        }
        let others = self.platforms.iter().filter(|listed| {
            replaced.is_none_or(|old| !old.definition.is_named(&listed.definition.name))
        });
        for other in others {
            let taken = std::iter::once(&definition.name)
                .chain(&definition.other_names)
                .find(|name| other.definition.is_named(name) || other.definition.answers_to(name));
            if let Some(name) = taken {
                return Err(EditError::AlreadyExists(format!(
                    "the name {name} (platform {})",
                    other.definition.name
                )));
            }
        }
        let previous = replaced.map_or(definition.name.clone(), |old| old.definition.name.clone());
        self.edit_file(PLATFORMS_FILE.into(), |document| {
            platforms::write(document, &previous, &definition);
            Ok(())
        })
    }

    /// Deletes the workspace's platform definition called `name` (ignoring
    /// case), or for a built-in definition the workspace changes, its
    /// changes (a reset). A built-in definition as built in cannot be
    /// deleted.
    pub fn delete_platform(&mut self, name: &str) -> Result<(), EditError> {
        let listed = self
            .listed_platform(name)
            .ok_or_else(|| EditError::NotFound(format!("platform {}", name.trim())))?;
        if listed.source == PlatformSource::BuiltIn {
            return Err(EditError::Invalid(format!(
                "the platform {} is built in and unchanged",
                listed.definition.name
            )));
        }
        let name = listed.definition.name.clone();
        self.edit_file(PLATFORMS_FILE.into(), |document| {
            platforms::remove(document, &name);
            Ok(())
        })
    }

    fn listed_platform(&self, name: &str) -> Option<&ListedPlatform> {
        self.platforms
            .iter()
            .find(|listed| listed.definition.is_named(name))
    }

    /// The snippets, in the order of the snippets file.
    pub fn snippets(&self) -> &[Snippet] {
        &self.snippets
    }

    /// The snippet called `name` (ignoring case).
    pub fn snippet(&self, name: &str) -> Option<&Snippet> {
        self.snippets
            .iter()
            .find(|snippet| snippets::same(&snippet.name, name))
    }

    /// The snippets having every word of `query` in their name, group or
    /// text (ignoring case): those with all of them in the name first, each
    /// part in file order. An empty query gives them all.
    pub fn find_snippets(&self, query: &str) -> Vec<&Snippet> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut found: Vec<(bool, &Snippet)> = self
            .snippets
            .iter()
            .filter_map(|snippet| snippet.matches(&words).map(|by_name| (by_name, snippet)))
            .collect();
        found.sort_by_key(|(by_name, _)| !by_name);
        found.into_iter().map(|(_, snippet)| snippet).collect()
    }

    /// Saves `snippet` to the snippets file, in place of the snippet called
    /// `previous` (ignoring case), or as a new one after the others when
    /// `previous` is `None`. Its name and group are trimmed (a blank group
    /// is none). A snippet without a name or text, or with the name of
    /// another snippet, is refused.
    pub fn save_snippet(
        &mut self,
        previous: Option<&str>,
        snippet: &Snippet,
    ) -> Result<(), EditError> {
        let snippet = snippets::clean(snippet).map_err(|message| {
            EditError::Invalid(format!("the snippet cannot be saved: {message}"))
        })?;
        let replaced = match previous {
            Some(previous) => Some(
                self.snippet(previous)
                    .ok_or_else(|| EditError::NotFound(format!("snippet {}", previous.trim())))?
                    .name
                    .clone(),
            ),
            None => None,
        };
        let clash = self.snippets.iter().any(|other| {
            snippets::same(&other.name, &snippet.name)
                && replaced
                    .as_deref()
                    .is_none_or(|old| !snippets::same(old, &other.name))
        });
        if clash {
            return Err(EditError::AlreadyExists(format!(
                "snippet {}",
                snippet.name
            )));
        }
        self.edit_file(SNIPPETS_FILE.into(), |document| {
            snippets::write(document, replaced.as_deref(), &snippet)
                .map_err(|name| EditError::AlreadyExists(format!("snippet {name}")))
        })
    }

    /// Deletes the snippet called `name` (ignoring case); the file goes when
    /// no snippet is left.
    pub fn delete_snippet(&mut self, name: &str) -> Result<(), EditError> {
        self.edit_file(SNIPPETS_FILE.into(), |document| {
            if snippets::remove(document, name) {
                Ok(())
            } else {
                Err(EditError::NotFound(format!("snippet {}", name.trim())))
            }
        })
    }

    /// The lines to send for the snippet called `name` to a session with
    /// `host` (`None`: a session to no host of the workspace). Its variables
    /// are filled from the host's fields ([`ResolvedHost::variables`]), the
    /// rest from `answers`; without values for all of them, which ones are
    /// missing ([`ExpandError::Missing`]), for the user to be asked.
    pub fn expand_snippet(
        &self,
        name: &str,
        host: Option<&HostId>,
        answers: &Variables,
    ) -> Result<Vec<String>, ExpandError> {
        let snippet = self
            .snippet(name)
            .ok_or_else(|| ExpandError::NoSuchSnippet(name.to_string()))?;
        let mut values = match host {
            Some(id) => self
                .resolve_host(id)
                .ok_or_else(|| ExpandError::NoSuchHost(id.clone()))?
                .variables(),
            None => Variables::new(),
        };
        values.fill_gaps(answers);
        snippet.expand(&values).map_err(ExpandError::Missing)
    }

    /// The snippet `name` ready to send to each member of the multi-send
    /// group (#36), in their order: each filled with its own host's values
    /// (`hosts`; `None` for a member without a live host, which gets only
    /// the answers), then `answers` for the gaps. If any member still lacks
    /// a value, every variable some member lacks is reported once, in the
    /// snippet's order, so the user answers once for the whole group.
    pub fn expand_snippet_for_each(
        &self,
        name: &str,
        hosts: &[Option<HostId>],
        answers: &Variables,
    ) -> Result<Vec<Vec<String>>, ExpandError> {
        let mut expanded = Vec::with_capacity(hosts.len());
        let mut missing: Vec<String> = Vec::new();
        for host in hosts {
            match self.expand_snippet(name, host.as_ref(), answers) {
                Ok(lines) => expanded.push(lines),
                Err(ExpandError::Missing(lacking)) => missing.extend(lacking.names),
                Err(error) => return Err(error),
            }
        }
        if missing.is_empty() {
            return Ok(expanded);
        }
        let snippet = self
            .snippet(name)
            .ok_or_else(|| ExpandError::NoSuchSnippet(name.to_string()))?;
        let names = snippet
            .variables()
            .into_iter()
            .filter(|variable| missing.contains(variable))
            .collect();
        Err(ExpandError::Missing(MissingVariables { names }))
    }

    /// Files that could not be loaded, ordered by path.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The host with this id, if the workspace has it.
    pub fn host(&self, id: &HostId) -> Option<&Host> {
        let (folder, _) = id.split();
        self.folder(folder)?
            .hosts
            .iter()
            .find(|host| &host.id == id)
    }

    /// The folder at this path (empty for the root), if the workspace has it.
    pub fn folder(&self, path: &str) -> Option<&Folder> {
        if path.is_empty() {
            return Some(&self.tree);
        }
        path.split('/').try_fold(&self.tree, |folder, name| {
            folder.folders.iter().find(|child| child.name == name)
        })
    }

    /// The host `id` with the settings that apply to it: its own, else its
    /// folder's, else each parent's up to the root, else the defaults.
    pub fn resolve_host(&self, id: &HostId) -> Option<ResolvedHost> {
        let host = self.host(id)?;
        let (folder, _) = id.split();
        let mut levels = self.levels(folder)?;
        levels.push(Level {
            origin: Origin::Host,
            settings: &host.settings,
            tags: &host.tags,
        });
        Some(ResolvedHost {
            host: host.clone(),
            settings: settings::resolve(&levels),
        })
    }

    /// The settings that apply to the folder at `path`, and so to what is
    /// below it unless overridden there.
    pub fn resolve_folder(&self, path: &str) -> Option<EffectiveSettings> {
        Some(settings::resolve(&self.levels(path)?))
    }

    /// The folders from the root down to the one at `path`.
    fn levels(&self, path: &str) -> Option<Vec<Level<'_>>> {
        let mut folder = &self.tree;
        let mut levels = vec![Level::of(folder)];
        if !path.is_empty() {
            for name in path.split('/') {
                folder = folder.folders.iter().find(|child| child.name == name)?;
                levels.push(Level::of(folder));
            }
        }
        Some(levels)
    }

    /// Creates folder `name` inside the folder at `parent`; returns its path.
    pub fn create_folder(&mut self, parent: &str, name: &str) -> Result<String, EditError> {
        self.existing_folder(parent)?;
        let name = check_name(name)?;
        let path = join(parent, name);
        self.ensure_free(parent, Entry::Folder(name), None)?;
        fs::create_dir(self.root.join(&path)).map_err(io_error(&path))?;
        self.reload()?;
        Ok(path)
    }

    /// Renames the folder at `path`; returns its new path.
    pub fn rename_folder(&mut self, path: &str, name: &str) -> Result<String, EditError> {
        self.existing_subfolder(path, "renamed")?;
        let name = check_name(name)?;
        let (parent, old_name) = split_path(path);
        if name == old_name {
            return Ok(path.to_string());
        }
        let new_path = join(parent, name);
        self.ensure_free(parent, Entry::Folder(name), Some(old_name))?;
        self.rename(path, &new_path)?;
        Ok(new_path)
    }

    /// Moves the folder at `path`, with everything in it, into the folder at
    /// `parent`; returns its new path.
    pub fn move_folder(&mut self, path: &str, parent: &str) -> Result<String, EditError> {
        self.existing_subfolder(path, "moved")?;
        self.existing_folder(parent)?;
        if parent == path || parent.starts_with(&format!("{path}/")) {
            return Err(EditError::Invalid(format!(
                "{path} cannot be moved into itself"
            )));
        }
        let (old_parent, name) = split_path(path);
        if old_parent == parent {
            return Ok(path.to_string());
        }
        let new_path = join(parent, name);
        self.ensure_free(parent, Entry::Folder(name), None)?;
        self.rename(path, &new_path)?;
        Ok(new_path)
    }

    /// Deletes the folder at `path` and everything in it, files the app does
    /// not show included. A folder holding hosts is only deleted with
    /// [`FolderDeletion::IncludingHosts`].
    pub fn delete_folder(&mut self, path: &str, deletion: FolderDeletion) -> Result<(), EditError> {
        let hosts = self.existing_subfolder(path, "deleted")?.host_count();
        if hosts > 0 && deletion == FolderDeletion::IfNoHosts {
            return Err(EditError::FolderHasHosts {
                folder: path.to_string(),
                hosts,
            });
        }
        fs::remove_dir_all(self.root.join(path)).map_err(io_error(path))?;
        self.reload()
    }

    /// Sets (or with `None` or blank text, removes) the description of the
    /// folder at `path`, keeping its other settings. A settings file left
    /// empty is removed.
    pub fn set_folder_description(
        &mut self,
        path: &str,
        description: Option<&str>,
    ) -> Result<(), EditError> {
        let current = self.existing_folder(path)?.description.clone();
        let description = normalize(description);
        if description == current {
            return Ok(());
        }
        self.edit_folder_file(path, |document| {
            match description {
                Some(text) => set_folder_key(document, DESCRIPTION, text.into()),
                None => {
                    document.shift_remove(DESCRIPTION);
                }
            }
            Ok(())
        })
    }

    /// Sets the tags of the folder at `path`, which every host below it
    /// carries too. Blank and repeated tags are dropped.
    pub fn set_folder_tags(&mut self, path: &str, tags: &[String]) -> Result<(), EditError> {
        let current = self.existing_folder(path)?.tags.clone();
        let tags = clean_tags(tags);
        if tags == current {
            return Ok(());
        }
        self.edit_folder_file(path, |document| {
            write_tags(document, tags, set_folder_key);
            Ok(())
        })
    }

    /// Saves the settings the folder at `path` sets for everything below it
    /// (`None` and empty lists clear them), keeping the rest of its settings
    /// file. Text is trimmed and blank text clears a setting; only changed
    /// settings are touched.
    pub fn write_folder_settings(&mut self, path: &str, edit: &Settings) -> Result<(), EditError> {
        let current = self.existing_folder(path)?.settings.clone();
        self.edit_folder_file(path, |document| {
            apply_settings(document, &current, edit, set_folder_key)
        })
    }

    /// Changes the settings file of the folder at `path` (see
    /// [`Self::edit_file`]).
    fn edit_folder_file(
        &mut self,
        path: &str,
        change: impl FnOnce(&mut Mapping) -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        self.existing_folder(path)?;
        self.edit_file(join(path, FOLDER_SETTINGS_FILE), change)
    }

    /// Changes the mapping in `file` (relative to the root; a missing file
    /// is an empty mapping): it is written if the change made a difference,
    /// and removed if left empty.
    fn edit_file(
        &mut self,
        file: String,
        change: impl FnOnce(&mut Mapping) -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        let full = self.root.join(&file);
        let original = if full.exists() {
            read_mapping(&full).map_err(|message| {
                EditError::Invalid(format!("{file} cannot be updated: {message}"))
            })?
        } else {
            Mapping::new()
        };
        let mut document = original.clone();
        change(&mut document)?;
        if document == original {
            return Ok(());
        }
        if document.is_empty() {
            if full.exists() {
                fs::remove_file(&full).map_err(io_error(&file))?;
            }
        } else {
            write_mapping(&self.root, &file, &document)?;
        }
        self.reload()
    }

    /// Creates host `name` with its management address in the folder at
    /// `folder`; returns its id.
    pub fn create_host(
        &mut self,
        folder: &str,
        name: &str,
        address: &str,
    ) -> Result<HostId, EditError> {
        self.existing_folder(folder)?;
        let name = check_name(name)?;
        let address = required_address(address)?;
        self.ensure_free(folder, Entry::Host(name), None)?;
        let file = join(folder, &format!("{name}.{HOST_EXTENSION}"));
        let mut document = Mapping::new();
        document.insert(ADDRESS.into(), address.into());
        write_mapping(&self.root, &file, &document)?;
        self.reload()?;
        Ok(HostId::new(file))
    }

    /// Renames the host `id`: its file (keeping the extension) and, if the
    /// file names it explicitly, its `name`. Returns its new id.
    pub fn rename_host(&mut self, id: &HostId, name: &str) -> Result<HostId, EditError> {
        let host = self.existing_host(id)?.clone();
        let name = check_name(name)?;
        let (folder, file_name) = id.split();
        let extension = file_name
            .rsplit_once('.')
            .map_or(HOST_EXTENSION, |(_, e)| e);
        let new_id = HostId::new(join(folder, &format!("{name}.{extension}")));
        if new_id != *id {
            self.ensure_free(folder, Entry::Host(name), Some(file_name))?;
            fs::rename(self.root.join(id.as_str()), self.root.join(new_id.as_str()))
                .map_err(io_error(id.as_str()))?;
        }
        if host.document.contains_key(NAME) && host.name != name {
            let mut document = host.document;
            document.insert(NAME.into(), name.into());
            write_mapping(&self.root, new_id.as_str(), &document)?;
        }
        self.reload()?;
        Ok(new_id)
    }

    /// Moves the host `id` into the folder at `folder`; its file is moved
    /// unchanged. Returns its new id.
    pub fn move_host(&mut self, id: &HostId, folder: &str) -> Result<HostId, EditError> {
        self.existing_host(id)?;
        self.existing_folder(folder)?;
        let (old_folder, file_name) = id.split();
        if old_folder == folder {
            return Ok(id.clone());
        }
        let stem = yaml_stem(file_name).unwrap_or(file_name);
        self.ensure_free(folder, Entry::Host(stem), None)?;
        let new_id = HostId::new(join(folder, file_name));
        self.rename(id.as_str(), new_id.as_str())?;
        Ok(new_id)
    }

    /// Deletes the host `id`'s file.
    pub fn delete_host(&mut self, id: &HostId) -> Result<(), EditError> {
        self.existing_host(id)?;
        fs::remove_file(self.root.join(id.as_str())).map_err(io_error(id.as_str()))?;
        self.reload()
    }

    /// Saves `host`'s fields to its file (to rename or move it, see
    /// [`Self::rename_host`] and [`Self::move_host`]).
    ///
    /// Text is trimmed and blank text clears a field; blank and repeated
    /// tags and empty custom rows are dropped. Keys of the file that the
    /// host does not model are kept, in their original order, and unchanged
    /// fields keep their exact value; comments are not kept. If nothing
    /// changed, the file is not touched at all.
    pub fn write_host(&mut self, host: &Host) -> Result<(), EditError> {
        let current = self.existing_host(&host.id)?;
        let document = updated_document(current, host)?;
        if document == current.document {
            return Ok(());
        }
        write_mapping(&self.root, host.id.as_str(), &document)?;
        self.reload()
    }

    /// Loads the workspace again from its files, after changes made outside
    /// the app (see [`WorkspaceWatcher`]). Returns whether anything differs
    /// from what was loaded before: the tree, a host file's content, the
    /// roles, the filters or the problems. After the app's own edits it
    /// finds nothing, as each edit already reloaded. If the folder can no
    /// longer be opened, the workspace is left as it was.
    pub fn refresh(&mut self) -> Result<bool, OpenError> {
        let fresh = Self::open(&self.root)?;
        let changed = fresh.tree != self.tree
            || fresh.id != self.id
            || fresh.roles != self.roles
            || fresh.filters != self.filters
            || fresh.highlights != self.highlights
            || fresh.platforms != self.platforms
            || fresh.snippets != self.snippets
            || fresh.problems != self.problems;
        if changed {
            self.replace_with(fresh);
        }
        Ok(changed)
    }

    fn reload(&mut self) -> Result<(), EditError> {
        let fresh = Self::open(&self.root).map_err(|error| EditError::Io {
            file: ".".into(),
            source: io::Error::other(error.reason),
        })?;
        self.replace_with(fresh);
        Ok(())
    }

    /// Takes the freshly loaded `fresh` as the next revision of this one.
    fn replace_with(&mut self, fresh: Self) {
        let revision = self.revision + 1;
        *self = Self { revision, ..fresh };
    }

    fn existing_folder(&self, path: &str) -> Result<&Folder, EditError> {
        self.folder(path)
            .ok_or_else(|| EditError::NotFound(path.to_string()))
    }

    /// A folder other than the root, which cannot be `what` from inside.
    fn existing_subfolder(&self, path: &str, what: &str) -> Result<&Folder, EditError> {
        if path.is_empty() {
            return Err(EditError::Invalid(format!(
                "the workspace folder itself cannot be {what} here"
            )));
        }
        self.existing_folder(path)
    }

    fn existing_host(&self, id: &HostId) -> Result<&Host, EditError> {
        self.host(id)
            .ok_or_else(|| EditError::NotFound(id.to_string()))
    }

    /// Fails if the folder at `folder` already has an entry that `entry`
    /// would clash with, other than `except` (the entry being renamed).
    /// Names are compared ignoring case, as on Windows and macOS.
    fn ensure_free(
        &self,
        folder: &str,
        entry: Entry<'_>,
        except: Option<&str>,
    ) -> Result<(), EditError> {
        let dir = self.root.join(folder);
        let entries = fs::read_dir(&dir).map_err(io_error(folder))?;
        let (name, file_name) = match entry {
            Entry::Folder(name) => (name, None),
            Entry::Host(name) => (name, Some(format!("{name}.{HOST_EXTENSION}"))),
        };
        let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
        for existing in entries {
            let existing = existing.map_err(io_error(folder))?;
            let existing_name = existing.file_name().to_string_lossy().into_owned();
            if Some(existing_name.as_str()) == except {
                continue;
            }
            let clash = match &file_name {
                None => same(&existing_name, name),
                Some(file_name) => {
                    same(&existing_name, file_name)
                        || (existing.path().is_file()
                            && yaml_stem(&existing_name).is_some_and(|stem| same(stem, name)))
                }
            };
            if clash {
                return Err(EditError::AlreadyExists(join(folder, &existing_name)));
            }
        }
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), EditError> {
        fs::rename(self.root.join(from), self.root.join(to)).map_err(io_error(from))?;
        self.reload()
    }
}

impl<'a> Level<'a> {
    fn of(folder: &'a Folder) -> Self {
        Self {
            origin: Origin::Folder {
                path: folder.path.clone(),
            },
            settings: &folder.settings,
            tags: &folder.tags,
        }
    }
}

/// What is about to be created under a name.
enum Entry<'a> {
    Folder(&'a str),
    Host(&'a str),
}

fn io_error(file: &str) -> impl FnOnce(io::Error) -> EditError + '_ {
    move |source| EditError::Io {
        file: if file.is_empty() { "." } else { file }.to_string(),
        source,
    }
}

/// `parent/name`, or `name` at the root.
pub(crate) fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// The parent path and the last name of a path.
fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// Whether a file name cannot hold `c` on some system.
pub(crate) fn forbidden_in_file_names(c: char) -> bool {
    c.is_control() || "/\\:*?\"<>|".contains(c)
}

/// Whether Windows takes the file name `name` (with any extension) for a
/// device.
pub(crate) fn is_device_name(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// A folder or host name, trimmed, if it is a file name that works on every
/// system and that the workspace format does not reserve.
fn check_name(name: &str) -> Result<&str, EditError> {
    let name = name.trim();
    let reason = if name.is_empty() {
        Some("it is empty")
    } else if name.starts_with('.') {
        Some("names starting with `.` are hidden")
    } else if name.starts_with('_') {
        Some("names starting with `_` are reserved for workspace settings")
    } else if name.chars().any(forbidden_in_file_names) {
        Some("file names cannot contain / \\ : * ? \" < > |")
    } else if name.ends_with('.') {
        Some("file names cannot end with a dot")
    } else if is_device_name(name) {
        Some("it is a reserved device name on Windows")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EditError::InvalidName {
            name: name.to_string(),
            reason,
        }),
        None => Ok(name),
    }
}

fn required_address(address: &str) -> Result<&str, EditError> {
    let address = address.trim();
    if address.is_empty() {
        return Err(EditError::Invalid("the management address is empty".into()));
    }
    Ok(address)
}

/// Trimmed text; blank is none.
fn normalize(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// `current`'s file with `edit`'s fields applied. Values that did not change
/// keep their original YAML form.
fn updated_document(current: &Host, edit: &Host) -> Result<Mapping, EditError> {
    let mut document = current.document.clone();

    let name = edit.name.trim();
    if name.is_empty() {
        return Err(EditError::Invalid("the name is empty".into()));
    }
    // A name taken from the file name stays implicit until it changes.
    if name != current.name {
        set_key(&mut document, NAME, name.into());
    }

    let address = required_address(&edit.address)?;
    if address != current.address {
        set_key(&mut document, ADDRESS, address.into());
    }

    apply_settings(&mut document, &current.settings, &edit.settings, set_key)?;

    for ((key, old), (_, new)) in current.text_fields().into_iter().zip(edit.text_fields()) {
        let new = normalize(new.as_deref());
        if new == *old {
            continue;
        }
        match new {
            None => {
                document.shift_remove(key);
            }
            Some(text) => {
                // `007` stays text, so the value reads back as typed.
                let value = match text.parse::<u64>() {
                    Ok(number) if key == RACK_UNIT && number.to_string() == text => number.into(),
                    _ => text.into(),
                };
                set_key(&mut document, key, value);
            }
        }
    }

    let tags = clean_tags(&edit.tags);
    if tags != current.tags {
        write_tags(&mut document, tags, set_key);
    }

    let mut custom: Vec<CustomField> = Vec::new();
    for field in &edit.custom {
        let field = CustomField::new(field.key.trim(), field.value.trim());
        if field.key.is_empty() {
            if field.value.is_empty() {
                continue;
            }
            return Err(EditError::Invalid(
                "a custom field has a value but no name".into(),
            ));
        }
        if custom.iter().any(|seen| seen.key == field.key) {
            return Err(EditError::Invalid(format!(
                "custom field `{}` appears twice",
                field.key
            )));
        }
        custom.push(field);
    }
    if custom != current.custom {
        if custom.is_empty() {
            document.shift_remove(CUSTOM);
        } else {
            let old = current.document.get(CUSTOM).and_then(Value::as_mapping);
            let mapping = custom
                .into_iter()
                .map(|field| {
                    let unchanged = current.custom.contains(&field);
                    let value = old
                        .filter(|_| unchanged)
                        .and_then(|old| old.get(field.key.as_str()))
                        .cloned()
                        .unwrap_or_else(|| field.value.into());
                    (field.key.into(), value)
                })
                .collect();
            set_key(&mut document, CUSTOM, Value::Mapping(mapping));
        }
    }

    Ok(document)
}

/// Trimmed tags without blanks or repeats.
fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut clean: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if !tag.is_empty() && !clean.iter().any(|seen| seen == tag) {
            clean.push(tag.to_string());
        }
    }
    clean
}

/// Writes `tags`, or removes the key if there are none.
fn write_tags(document: &mut Mapping, tags: Vec<String>, set: fn(&mut Mapping, &str, Value)) {
    if tags.is_empty() {
        document.shift_remove(TAGS);
    } else {
        let list = tags.into_iter().map(Value::from).collect();
        set(document, TAGS, Value::Sequence(list));
    }
}

/// [`set_key_in`] for host files.
fn set_key(document: &mut Mapping, key: &str, value: Value) {
    set_key_in(document, key, value, &HOST_KEY_ORDER);
}

/// [`set_key_in`] for folder settings files.
fn set_folder_key(document: &mut Mapping, key: &str, value: Value) {
    set_key_in(document, key, value, &FOLDER_KEY_ORDER);
}

/// Sets `key` in place if the document has it; otherwise inserts it after
/// the nearest key before it in `order` that the document has (or before
/// the nearest one after it, or at the end).
fn set_key_in(document: &mut Mapping, key: &str, value: Value, order: &[&str]) {
    if let Some(slot) = document.get_mut(key) {
        *slot = value;
        return;
    }
    let rank = |key: &Value| {
        key.as_str()
            .and_then(|key| order.iter().position(|known| *known == key))
    };
    let mut entries: Vec<(Value, Value)> = std::mem::take(document).into_iter().collect();
    let position = order
        .iter()
        .position(|known| *known == key)
        .and_then(|mine| {
            let ranked = entries
                .iter()
                .enumerate()
                .filter_map(|(index, (key, _))| rank(key).map(|rank| (rank, index)));
            let before = ranked.clone().filter(|(rank, _)| *rank < mine).max();
            let after = ranked.filter(|(rank, _)| *rank > mine).min();
            before
                .map(|(_, index)| index + 1)
                .or(after.map(|(_, index)| index))
        })
        .unwrap_or(entries.len());
    entries.insert(position, (key.into(), value));
    *document = entries.into_iter().collect();
}

/// Writes a YAML mapping to `file` (relative to `root`): beside the file
/// first, then over it, so a failure never leaves a half-written file.
fn write_mapping(root: &Path, file: &str, document: &Mapping) -> Result<(), EditError> {
    let text = serde_norway::to_string(document).map_err(|error| EditError::Io {
        file: file.to_string(),
        source: io::Error::other(error),
    })?;
    let path = root.join(file);
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    fs::write(&temporary, text)
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|error| {
            let _ = fs::remove_file(&temporary);
            io_error(file)(error)
        })
}

/// The built-in roles with those of the workspace's roles file, if it has
/// one. A bad file or role definition becomes a problem; the rest load.
fn load_roles(root: &Path, problems: &mut Vec<Problem>) -> Vec<Role> {
    let path = root.join(ROLES_FILE);
    if !path.is_file() {
        return roles::built_in();
    }
    let problem = |message| Problem {
        file: ROLES_FILE.into(),
        message,
    };
    match read_mapping(&path) {
        Ok(document) => {
            let (roles, errors) = roles::with_workspace_roles(&document);
            problems.extend(errors.into_iter().map(problem));
            roles
        }
        Err(message) => {
            problems.push(problem(message));
            roles::built_in()
        }
    }
}

/// The workspace's highlight rules and the built-in ones. A bad file or rule
/// definition becomes a problem; the rest load.
fn load_highlights(root: &Path, problems: &mut Vec<Problem>) -> Vec<ListedRule> {
    let path = root.join(HIGHLIGHTS_FILE);
    if !path.is_file() {
        return crate::highlight::built_in_rules();
    }
    let problem = |message| Problem {
        file: HIGHLIGHTS_FILE.into(),
        message,
    };
    match read_mapping(&path) {
        Ok(document) => {
            let (rules, errors) = highlights::load(&document);
            problems.extend(errors.into_iter().map(problem));
            rules
        }
        Err(message) => {
            problems.push(problem(message));
            crate::highlight::built_in_rules()
        }
    }
}

/// The built-in platform definitions with those of the workspace's
/// platforms file, if it has one. A bad file or definition becomes a
/// problem; the rest load.
fn load_platforms(root: &Path, problems: &mut Vec<Problem>) -> Vec<ListedPlatform> {
    let path = root.join(PLATFORMS_FILE);
    if !path.is_file() {
        return crate::platform::built_in_platforms();
    }
    let problem = |message| Problem {
        file: PLATFORMS_FILE.into(),
        message,
    };
    match read_mapping(&path) {
        Ok(document) => {
            let (platforms, errors) = platforms::load(&document);
            problems.extend(errors.into_iter().map(problem));
            platforms
        }
        Err(message) => {
            problems.push(problem(message));
            crate::platform::built_in_platforms()
        }
    }
}

fn compile_highlights(rules: &[ListedRule]) -> Highlighter {
    let usable: Vec<HighlightRule> = rules
        .iter()
        .filter(|listed| listed.error.is_none())
        .map(|listed| listed.rule.clone())
        .collect();
    Highlighter::new(&usable)
}

/// The workspace's snippets, if it has a snippets file. A bad file or
/// snippet definition becomes a problem; the rest load.
fn load_snippets(root: &Path, problems: &mut Vec<Problem>) -> Vec<Snippet> {
    let path = root.join(SNIPPETS_FILE);
    if !path.is_file() {
        return Vec::new();
    }
    let problem = |message| Problem {
        file: SNIPPETS_FILE.into(),
        message,
    };
    match read_mapping(&path) {
        Ok(document) => {
            let (snippets, errors) = snippets::load(&document);
            problems.extend(errors.into_iter().map(problem));
            snippets
        }
        Err(message) => {
            problems.push(problem(message));
            Vec::new()
        }
    }
}

/// The workspace's saved filters, if it has a filters file. A bad file or
/// filter definition becomes a problem; the rest load.
fn load_filters(root: &Path, problems: &mut Vec<Problem>) -> Vec<SavedFilter> {
    let path = root.join(FILTERS_FILE);
    if !path.is_file() {
        return Vec::new();
    }
    let problem = |message| Problem {
        file: FILTERS_FILE.into(),
        message,
    };
    match read_mapping(&path) {
        Ok(document) => {
            let (filters, errors) = filters::load(&document);
            problems.extend(errors.into_iter().map(problem));
            filters
        }
        Err(message) => {
            problems.push(problem(message));
            Vec::new()
        }
    }
}

fn load_folder(dir: &Path, name: String, path: String, problems: &mut Vec<Problem>) -> Folder {
    let mut folder = Folder {
        name,
        path: path.clone(),
        description: None,
        tags: Vec::new(),
        settings: Settings::default(),
        folders: Vec::new(),
        hosts: Vec::new(),
    };
    let problem = |file: &str, message: String| Problem {
        file: if file.is_empty() {
            ".".into()
        } else {
            file.into()
        },
        message,
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(problem(&path, format!("cannot read folder: {error}")));
            return folder;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                problems.push(problem(&path, format!("cannot read folder: {error}")));
                continue;
            }
        };
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        if entry_name.starts_with('.') {
            continue;
        }
        let entry_path = join(&path, &entry_name);
        let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        let is_dir = fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir());

        if is_dir && is_link {
            // A link can point back up the tree; folders are real directories.
            problems.push(problem(
                &entry_path,
                "links to folders are not followed".into(),
            ));
        } else if is_dir {
            let child = load_folder(&entry.path(), entry_name, entry_path, problems);
            folder.folders.push(child);
        } else if entry_name == FOLDER_SETTINGS_FILE {
            match read_mapping(&entry.path()).and_then(|document| {
                Ok((
                    optional_text(&document, DESCRIPTION)?,
                    read_tags(&document)?,
                    settings_from_document(&document)?,
                ))
            }) {
                Ok((description, tags, settings)) => {
                    folder.description = description;
                    folder.tags = tags;
                    folder.settings = settings;
                }
                Err(message) => problems.push(problem(&entry_path, message)),
            }
        } else if entry_name.starts_with('_') {
            // Reserved for workspace-wide definitions.
        } else if let Some(stem) = yaml_stem(&entry_name) {
            match read_mapping(&entry.path()).and_then(|document| {
                host_from_document(HostId::new(entry_path.clone()), stem, document)
            }) {
                Ok(host) => folder.hosts.push(host),
                Err(message) => problems.push(problem(&entry_path, message)),
            }
        }
    }

    folder.folders.sort_by(|a, b| by_name(&a.name, &b.name));
    folder
        .hosts
        .sort_by(|a, b| by_name(&a.name, &b.name).then_with(|| a.id.cmp(&b.id)));
    folder
}

/// Case-insensitive, with exact order as the tie-break.
fn by_name(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The file name without its YAML extension, if it is a YAML file.
fn yaml_stem(file_name: &str) -> Option<&str> {
    let (stem, extension) = file_name.rsplit_once('.')?;
    let is_yaml = extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml");
    (is_yaml && !stem.is_empty()).then_some(stem)
}

/// Reads a YAML file whose top level must be a mapping. An empty file is an
/// empty mapping.
fn read_mapping(path: &Path) -> Result<Mapping, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))?;
    if text.trim().is_empty() {
        return Ok(Mapping::new());
    }
    match serde_norway::from_str::<Value>(&text) {
        Ok(Value::Mapping(mapping)) => Ok(mapping),
        Ok(Value::Null) => Ok(Mapping::new()),
        Ok(_) => Err("expected a mapping of keys to values at the top level".into()),
        Err(error) => Err(format!("invalid YAML: {error}")),
    }
}

fn host_from_document(id: HostId, stem: &str, document: Mapping) -> Result<Host, String> {
    let name = optional_text(&document, NAME)?.unwrap_or_else(|| stem.to_string());
    let address =
        optional_text(&document, ADDRESS)?.ok_or_else(|| format!("missing `{ADDRESS}`"))?;
    let settings = settings_from_document(&document)?;
    let field = |key| detail_text(&document, key);
    let tags = read_tags(&document)?;
    let custom_error = || format!("`{CUSTOM}` must map names to text");
    let custom = match document.get(CUSTOM) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Mapping(fields)) => fields
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    Value::Null => Some(String::new()),
                    value => scalar_text(value),
                };
                Some(CustomField::new(scalar_text(key)?, value?))
            })
            .collect::<Option<_>>()
            .ok_or_else(custom_error)?,
        Some(_) => return Err(custom_error()),
    };
    Ok(Host {
        id,
        name,
        address,
        vendor: field(VENDOR)?,
        model: field(MODEL)?,
        serial_number: field(SERIAL_NUMBER)?,
        site: field(SITE)?,
        building: field(BUILDING)?,
        room: field(ROOM)?,
        rack: field(RACK)?,
        rack_unit: field(RACK_UNIT)?,
        role: field(ROLE)?,
        description: field(DESCRIPTION)?,
        tags,
        custom,
        settings,
        document,
    })
}

fn read_tags(document: &Mapping) -> Result<Vec<String>, String> {
    let error = || format!("`{TAGS}` must be a list of text");
    match document.get(TAGS) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Sequence(items)) => items
            .iter()
            .map(scalar_text)
            .collect::<Option<_>>()
            .ok_or_else(error),
        Some(_) => Err(error()),
    }
}

/// A key whose value, when present, must be non-empty text (a number is
/// taken as its text).
fn optional_text(document: &Mapping, key: &str) -> Result<Option<String>, String> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim().to_string())),
        Some(Value::String(_)) => Err(format!("`{key}` is empty")),
        Some(Value::Number(number)) => Ok(Some(number.to_string())),
        Some(_) => Err(format!("`{key}` must be text")),
    }
}

/// An optional detail field: text, a number or a boolean, taken as its
/// trimmed text; blank is unset.
fn detail_text(document: &Mapping, key: &str) -> Result<Option<String>, String> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => scalar_text(value)
            .map(|text| normalize(Some(&text)))
            .ok_or_else(|| format!("`{key}` must be text")),
    }
}

/// The text of a scalar YAML value.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}
