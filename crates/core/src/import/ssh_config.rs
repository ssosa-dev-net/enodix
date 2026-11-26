//! OpenSSH config import: the `Host` entries of an `~/.ssh/config` file
//! (#26).
//!
//! Each name a `Host` line gives without a wildcard (`*`, `?`) or a `!`
//! becomes a host file in the target folder, named after it. It takes its
//! values as `ssh` does: the global lines before the first `Host`, then
//! every `Host` block whose patterns match the name, in file order, the
//! first value of each keyword winning. Imported:
//!
//! - `HostName` is the management address (the name itself without one,
//!   as for `ssh name`); `%h` in it stands for the name, `%%` for `%`;
//! - `Port` is the port setting;
//! - `ProxyJump` with one hop that is another name of the same file is the
//!   jump host setting (that host's file). A hop that is not a name of the
//!   file, several hops and `ProxyCommand` are reported instead.
//!
//! Never imported, each reported once per keyword as skipped: `User` and
//! `IdentityFile` (they belong in a credential profile), `Match` blocks,
//! `Include` (the included files are not read) and every other keyword.
//! Hosts that exist already are handled as in every import (see
//! [`crate::import`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{
    existing_hosts, write_planned, ImportReport, ImportedHost, Planned, PreviewOutcome, ReportEntry,
};
use crate::names;
use crate::workspace::import::check_import_name;
use crate::workspace::{bad_address, join, BadAddress, HostId, Workspace, HOST_EXTENSION};

/// A config file that cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum SshConfigError {
    #[error("cannot read the SSH config file: {0}")]
    Io(String),
}

/// An import that cannot run.
#[derive(Debug, thiserror::Error)]
pub enum SshConfigImportError {
    #[error("the target folder {0} is not in this workspace")]
    NoSuchFolder(String),
}

/// One `Keyword value` line.
#[derive(Debug, Clone)]
struct Line {
    /// 1-based.
    line: usize,
    /// As written, for the report.
    keyword: String,
    args: Vec<String>,
}

impl Line {
    fn is(&self, keyword: &str) -> bool {
        self.keyword.eq_ignore_ascii_case(keyword)
    }
}

/// Which names a block's lines apply to.
#[derive(Debug, Clone)]
enum Applies {
    /// The lines before the first `Host` or `Match`.
    All,
    /// A `Host` line's patterns (`!` ones excluding).
    Host(Vec<String>),
    /// A `Match` block: never applied.
    Match,
}

#[derive(Debug, Clone)]
struct Block {
    /// The line of its `Host` or `Match` (0 for the global block).
    line: usize,
    applies: Applies,
    lines: Vec<Line>,
}

/// An OpenSSH client config as read.
#[derive(Debug, Clone)]
pub struct SshConfig {
    blocks: Vec<Block>,
}

impl SshConfig {
    /// Reads the file at `path`.
    pub fn read(path: &Path) -> Result<Self, SshConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| SshConfigError::Io(super::read_reason(&error)))?;
        Ok(Self::parse(&text))
    }

    /// The user's own config file (`.ssh/config` in their home folder:
    /// `HOME`, else `USERPROFILE` on Windows), if it is there.
    pub fn default_path() -> Option<PathBuf> {
        let home = ["HOME", "USERPROFILE"]
            .into_iter()
            .filter_map(std::env::var_os)
            .find(|home| !home.is_empty())?;
        let path = PathBuf::from(home).join(".ssh").join("config");
        path.is_file().then_some(path)
    }

    /// Reads the text of a config file. A line it cannot make sense of is
    /// kept as a keyword without values (and reported as not imported).
    pub fn parse(text: &str) -> Self {
        // Notepad and other Windows editors may start the file with a
        // byte-order mark, which would hide the first keyword.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut blocks = vec![Block {
            line: 0,
            applies: Applies::All,
            lines: Vec::new(),
        }];
        for (index, raw) in text.lines().enumerate() {
            let Some(line) = parse_line(raw, index + 1) else {
                continue;
            };
            if line.is("Host") {
                blocks.push(Block {
                    line: line.line,
                    applies: Applies::Host(line.args),
                    lines: Vec::new(),
                });
            } else if line.is("Match") {
                blocks.push(Block {
                    line: line.line,
                    applies: Applies::Match,
                    lines: Vec::new(),
                });
            } else if let Some(block) = blocks.last_mut() {
                block.lines.push(line);
            }
        }
        Self { blocks }
    }

    /// The names to import, each with the line of the `Host` that first
    /// gives it, in file order; each once (ignoring case).
    fn names(&self) -> Vec<(String, usize)> {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for block in &self.blocks {
            let Applies::Host(patterns) = &block.applies else {
                continue;
            };
            for pattern in patterns {
                if pattern.starts_with('!') || pattern.contains(['*', '?']) {
                    continue;
                }
                if seen.insert(pattern.to_lowercase()) {
                    names.push((pattern.clone(), block.line));
                }
            }
        }
        names
    }

    /// The first value of each keyword (lower case) for `name`, as `ssh`
    /// resolves it, with its line.
    fn values_for(&self, name: &str) -> HashMap<String, &Line> {
        let mut values: HashMap<String, &Line> = HashMap::new();
        for block in &self.blocks {
            let applies = match &block.applies {
                Applies::All => true,
                Applies::Host(patterns) => host_matches(patterns, name),
                Applies::Match => false,
            };
            if !applies {
                continue;
            }
            for line in &block.lines {
                values.entry(line.keyword.to_lowercase()).or_insert(line);
            }
        }
        values
    }
}

/// One line as keyword and values: `Keyword value`, `Keyword=value`,
/// double quotes around a value with spaces; `None` for a blank line or a
/// comment.
fn parse_line(raw: &str, line: usize) -> Option<Line> {
    let text = raw.trim();
    if text.is_empty() || text.starts_with('#') {
        return None;
    }
    let end = text
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(text.len());
    let keyword = text[..end].to_string();
    let mut rest = text[end..].trim_start();
    if let Some(after) = rest.strip_prefix('=') {
        rest = after.trim_start();
    }
    Some(Line {
        line,
        keyword,
        args: split_args(rest),
    })
}

/// Values separated by spaces, a double-quoted one taken whole.
fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    args.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            c => {
                current.push(c);
                any = true;
            }
        }
    }
    if any {
        args.push(current);
    }
    args
}

/// Whether a `Host` line's patterns take `name`: one matches it and no
/// `!` one does.
fn host_matches(patterns: &[String], name: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        match pattern.strip_prefix('!') {
            Some(negated) if wildcard(negated, name) => return false,
            Some(_) => {}
            None => matched |= wildcard(pattern, name),
        }
    }
    matched
}

/// OpenSSH's patterns: `*` any run of characters, `?` one, ignoring case.
fn wildcard(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if let Some((star_p, star_n)) = star {
            p = star_p + 1;
            n = star_n + 1;
            star = Some((star_p, star_n + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// The keywords that give a host its values.
const HOSTNAME: &str = "hostname";
const PORT: &str = "port";
const PROXY_JUMP: &str = "proxyjump";

/// Why keywords that are read but not imported are left out.
fn skipped_reason(keyword: &str) -> &'static str {
    match keyword.to_lowercase().as_str() {
        "user" => "usernames belong in a credential profile; not imported",
        "identityfile" => {
            "key files belong in a credential profile (log in with a key file); not imported"
        }
        "proxycommand" => {
            "proxy commands are not run; set the host's Jump host instead; not imported"
        }
        "include" => "included files are not read; import each of them on its own",
        "match" => "Match blocks are not imported",
        _ => "no setting of enodix takes it; not imported",
    }
}

/// An SSH config and the folder its hosts go to.
#[derive(Debug, Clone)]
pub struct SshConfigImport {
    pub config: SshConfig,
    /// The workspace folder the hosts go to; empty for the root.
    pub target: String,
}

/// What [`SshConfigImport::run`] would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigPreview {
    /// One per name of the file, in file order.
    pub hosts: Vec<SshConfigPreviewHost>,
    /// What is never imported: one entry per keyword (its first line), and
    /// the `ProxyJump` values that could not become a jump host.
    pub skipped: Vec<ReportEntry>,
}

/// What would become of one name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigPreviewHost {
    /// The line of the `Host` that gives the name.
    pub line: Option<usize>,
    pub name: String,
    pub outcome: PreviewOutcome,
    /// The values the host would get; `None` when it fails.
    pub host: Option<ImportedHost>,
}

impl SshConfigImport {
    /// What importing would do, host by host. Writes nothing.
    pub fn preview(&self, workspace: &Workspace) -> Result<SshConfigPreview, SshConfigImportError> {
        if workspace.folder(&self.target).is_none() {
            return Err(SshConfigImportError::NoSuchFolder(self.target.clone()));
        }
        Ok(self.plan(workspace))
    }

    /// Imports the hosts. Hosts that exist already are skipped unless their
    /// id is in `replace` (the user confirmed them); then they take the
    /// imported values and keep the rest. The workspace is reloaded once at
    /// the end, if anything was written.
    pub fn run(
        &self,
        workspace: &mut Workspace,
        replace: &[HostId],
    ) -> Result<ImportReport, SshConfigImportError> {
        let preview = self.preview(workspace)?;
        let mut report = ImportReport {
            skipped: preview.skipped.clone(),
            ..ImportReport::default()
        };
        let planned: Vec<Planned<'_>> = preview
            .hosts
            .iter()
            .map(|host| Planned {
                line: host.line,
                name: &host.name,
                outcome: &host.outcome,
                host: host.host.as_ref(),
            })
            .collect();
        write_planned(workspace, &planned, replace, &mut report);
        Ok(report)
    }

    fn plan(&self, workspace: &Workspace) -> SshConfigPreview {
        let target = self.target.as_str();
        let existing = existing_hosts(workspace, target);
        let mut skipped = self.skipped_keywords();
        let mut hosts = Vec::new();
        // Each name's host id, for the jump hosts: by lower-case name.
        let mut ids: HashMap<String, HostId> = HashMap::new();
        // Each importable name's `ProxyJump`, filled in once every id is known.
        let mut jumps: Vec<(usize, &Line)> = Vec::new();

        for (name, line) in self.config.names() {
            let preview = |outcome, host| SshConfigPreviewHost {
                line: Some(line),
                name: name.clone(),
                outcome,
                host,
            };
            let values = self.config.values_for(&name);
            let host = match imported_host(&name, &values, target) {
                Ok(host) => host,
                Err(reason) => {
                    hosts.push(preview(PreviewOutcome::Fail { reason }, None));
                    continue;
                }
            };
            let key = names::folded(&host.name);
            let id = match existing.get(&key) {
                Some(id) => id.clone(),
                None => HostId::new(join(target, &format!("{}.{HOST_EXTENSION}", host.name))),
            };
            let outcome = match existing.get(&key) {
                Some(_) => PreviewOutcome::Exists { id: id.clone() },
                None => PreviewOutcome::Create { id: id.clone() },
            };
            ids.insert(key, id);
            if let Some(proxy) = values.get(PROXY_JUMP) {
                jumps.push((hosts.len(), *proxy));
            }
            hosts.push(preview(outcome, Some(host)));
        }

        for (index, proxy) in jumps {
            let host = &mut hosts[index];
            match jump_host(proxy, &ids, &host.name) {
                Ok((jump, dropped)) => {
                    if let Some(values) = &mut host.host {
                        values.jump_host = jump.map(|id| id.to_string());
                    }
                    if let Some(reason) = dropped {
                        skipped.push(ReportEntry {
                            line: Some(proxy.line),
                            name: format!("{}: ProxyJump {}", host.name, proxy.args.join(" ")),
                            host: None,
                            reason: Some(reason),
                        });
                    }
                }
                Err(reason) => skipped.push(ReportEntry {
                    line: Some(proxy.line),
                    name: format!("{}: ProxyJump {}", host.name, proxy.args.join(" ")),
                    host: None,
                    reason: Some(reason),
                }),
            }
        }
        SshConfigPreview { hosts, skipped }
    }

    /// Every keyword that is not imported, once, at its first line; `Match`
    /// blocks once each.
    fn skipped_keywords(&self) -> Vec<ReportEntry> {
        let mut seen = HashSet::new();
        let mut skipped = Vec::new();
        for block in &self.config.blocks {
            if let Applies::Match = block.applies {
                skipped.push(ReportEntry {
                    line: Some(block.line),
                    name: "Match".into(),
                    host: None,
                    reason: Some(skipped_reason("match").into()),
                });
                continue;
            }
            for line in &block.lines {
                let keyword = line.keyword.to_lowercase();
                if matches!(keyword.as_str(), HOSTNAME | PORT | PROXY_JUMP) {
                    continue;
                }
                if seen.insert(keyword) {
                    skipped.push(ReportEntry {
                        line: Some(line.line),
                        name: line.keyword.clone(),
                        host: None,
                        reason: Some(skipped_reason(&line.keyword).into()),
                    });
                }
            }
        }
        skipped
    }
}

/// The values `name` gets, or why it cannot be imported.
fn imported_host(
    name: &str,
    values: &HashMap<String, &Line>,
    target: &str,
) -> Result<ImportedHost, String> {
    check_import_name(name)?;
    let address = match values.get(HOSTNAME).and_then(|line| line.args.first()) {
        Some(written) => expand_hostname(written, name)?,
        None => name.to_string(),
    };
    match bad_address(&address) {
        None => {}
        Some(BadAddress::Space) => {
            return Err(format!(
                "the address has a space in it ({address}): write one host name or IP address"
            ))
        }
        Some(BadAddress::Port { host, port }) => {
            return Err(format!(
                "the HostName has a port after it ({address}): write {host} as the HostName and Port {port}"
            ))
        }
    }
    let port = match values.get(PORT).and_then(|line| line.args.first()) {
        None => None,
        Some(written) => match written.parse::<u16>() {
            Ok(port) if port > 0 => Some(port),
            _ => return Err(format!("Port {written} is not a port (1 to 65535)")),
        },
    };
    Ok(ImportedHost {
        name: name.to_string(),
        address,
        folder: target.to_string(),
        fields: Vec::new(),
        port,
        jump_host: None,
        tags: Vec::new(),
        custom: Vec::new(),
    })
}

/// A `HostName` value with `%h` (the name) and `%%` expanded; other tokens
/// are refused.
fn expand_hostname(written: &str, name: &str) -> Result<String, String> {
    let mut address = String::new();
    let mut chars = written.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            address.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => address.push_str(name),
            Some('%') => address.push('%'),
            other => {
                let token = other.map(|c| format!("%{c}")).unwrap_or_else(|| "%".into());
                return Err(format!(
                    "HostName {written} uses {token}, which the import does not expand"
                ));
            }
        }
    }
    Ok(address)
}

/// The jump host a `ProxyJump` line gives `name`: `None` for `none`, and
/// what of the hop was left out (a user or port, which the jump host's own
/// settings give instead); an error says why it cannot be imported.
fn jump_host(
    proxy: &Line,
    ids: &HashMap<String, HostId>,
    name: &str,
) -> Result<(Option<HostId>, Option<String>), String> {
    let value = proxy.args.join(" ");
    if value.eq_ignore_ascii_case("none") {
        return Ok((None, None));
    }
    if value.contains(',') {
        return Err(
            "more than one hop: give each jump host its own Jump host setting, the last hop here"
                .into(),
        );
    }
    if value.contains("://") {
        return Err("not a name of this file: set the host's Jump host by hand".into());
    }
    let (user, hop) = match value.rsplit_once('@') {
        Some((user, hop)) => (Some(user), hop),
        None => (None, value.as_str()),
    };
    let (hop, port) = match hop.rsplit_once(':') {
        Some((hop, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            (hop, Some(port))
        }
        _ => (hop, None),
    };
    if hop.eq_ignore_ascii_case(name) {
        return Err("it names the host itself".into());
    }
    let dropped = match (user, port) {
        (None, None) => None,
        (Some(user), None) => Some(format!("user {user}")),
        (None, Some(port)) => Some(format!("port {port}")),
        (Some(user), Some(port)) => Some(format!("user {user} and port {port}")),
    }
    .map(|what| {
        format!(
            "{what} not kept: the jump host {hop} logs in with its own credential profile and port"
        )
    });
    match ids.get(&names::folded(hop)) {
        Some(id) => Ok((Some(id.clone()), dropped)),
        None => Err(format!(
            "{hop} is not a Host of this file that is imported: set the host's Jump host by hand"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_as_openssh_does() {
        assert!(wildcard("*", "r1"));
        assert!(wildcard("r?", "R1"));
        assert!(wildcard("*.lab", "core1.lab"));
        assert!(wildcard("c*1*", "core1x"));
        assert!(!wildcard("*.lab", "core1.prod"));
        assert!(!wildcard("r?", "r10"));
        assert!(host_matches(&["*".into(), "!bastion".into()], "r1"));
        assert!(!host_matches(&["*".into(), "!bastion".into()], "Bastion"));
        assert!(!host_matches(&["!bastion".into()], "r1"));
    }

    #[test]
    fn lines_take_spaces_equals_and_quotes() {
        let line = parse_line("  IdentityFile = \"C:\\My Keys\\id\"  ", 3).unwrap();
        assert_eq!(line.keyword, "IdentityFile");
        assert_eq!(line.args, ["C:\\My Keys\\id"]);
        let line = parse_line("Port=2222", 1).unwrap();
        assert_eq!(
            (line.keyword.as_str(), line.args.as_slice()),
            ("Port", &["2222".to_string()][..])
        );
        assert!(parse_line("   # a comment", 1).is_none());
        assert!(parse_line("", 1).is_none());
    }
}
