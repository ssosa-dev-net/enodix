//! Writing imported hosts (see [`crate::import`]): many hosts in one go,
//! with one reload at the end, never over a file that is already there.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};

use serde_norway::Mapping;

use super::{
    check_name, io_error, join, updated_document, write_mapping, yaml_stem, CustomField, EditError,
    Host, HostId, Settings, Workspace, HOST_EXTENSION,
};
use crate::names;

/// The values an import sets on a host. `None` leaves a field as it is;
/// `Some` of blank text clears it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImportedValues {
    pub address: Option<String>,
    /// Built-in text fields by file key (`vendor`, `rack_unit`, ...).
    pub text: Vec<(&'static str, String)>,
    /// The port setting; `None` leaves it as it is.
    pub port: Option<u16>,
    pub tags: Option<Vec<String>>,
    /// Custom fields by key; blank values remove the field.
    pub custom: Vec<(String, String)>,
}

/// A host an import writes.
#[derive(Debug, Clone)]
pub(crate) enum HostWrite {
    /// A new host file `<name>.yaml` in the folder at `folder` (a workspace
    /// path; folders that do not exist yet are created).
    New {
        folder: String,
        name: String,
        values: ImportedValues,
    },
    /// The user confirmed that the existing host `id` takes the values.
    Update { id: HostId, values: ImportedValues },
}

/// What became of one [`HostWrite`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WriteOutcome {
    Created(HostId),
    Updated(HostId),
    /// A new host's file name was taken on disk by then (by this host, as
    /// another file name spelling it): nothing was written.
    Exists(HostId),
    Failed(String),
}

/// What [`Workspace::write_imported`] did.
pub(crate) struct Written {
    /// One per write, in order.
    pub outcomes: Vec<WriteOutcome>,
    /// The folders it created, parents first.
    pub created_folders: Vec<String>,
    /// Why the workspace could not be reloaded afterwards, if it could not.
    pub reload_error: Option<String>,
}

/// The entries of one directory, ignoring case: every name, and the host
/// file names by stem.
#[derive(Default)]
struct Entries {
    /// Lower-case name → the name on disk.
    names: HashMap<String, String>,
    hosts: HashMap<String, String>,
}

impl Entries {
    fn read(dir: &std::path::Path) -> io::Result<Self> {
        let mut entries = Self::default();
        match fs::read_dir(dir) {
            Ok(list) => {
                for entry in list {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.path().is_file() {
                        if let Some(stem) = yaml_stem(&name) {
                            entries.hosts.insert(names::folded(stem), name.clone());
                        }
                    }
                    entries.names.insert(names::folded(&name), name);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(entries)
    }

    /// The existing file a host called `name` would clash with.
    fn clash(&self, name: &str) -> Option<String> {
        let lower = names::folded(name);
        if let Some(file) = self.hosts.get(&lower) {
            return Some(file.clone());
        }
        self.names
            .get(&format!("{lower}.{HOST_EXTENSION}"))
            .cloned()
    }
}

/// Checks a host or folder name an import would create.
pub(crate) fn check_import_name(name: &str) -> Result<&str, String> {
    check_name(name).map_err(|error| error.to_string())
}

impl Workspace {
    /// Writes imported hosts, then reloads once (if anything was written).
    /// A new host whose file name is taken on disk meanwhile is not
    /// written ([`WriteOutcome::Exists`]).
    pub(crate) fn write_imported(&mut self, writes: Vec<HostWrite>) -> Written {
        let mut outcomes = Vec::with_capacity(writes.len());
        let mut created_folders = Vec::new();
        let mut listings: HashMap<String, Entries> = HashMap::new();
        let mut wrote = false;
        for write in writes {
            let outcome = match write {
                HostWrite::New {
                    folder,
                    name,
                    values,
                } => self.write_new(
                    &folder,
                    &name,
                    &values,
                    &mut listings,
                    &mut created_folders,
                    &mut wrote,
                ),
                HostWrite::Update { id, values } => match self.write_update(&id, &values) {
                    Ok(changed) => {
                        wrote |= changed;
                        WriteOutcome::Updated(id)
                    }
                    Err(error) => WriteOutcome::Failed(error.to_string()),
                },
            };
            outcomes.push(outcome);
        }
        // The files are written either way; the tree shows them on the
        // next refresh.
        let reload_error = if wrote {
            self.reload().err().map(|error| error.to_string())
        } else {
            None
        };
        Written {
            outcomes,
            created_folders,
            reload_error,
        }
    }

    fn write_new(
        &self,
        folder: &str,
        name: &str,
        values: &ImportedValues,
        listings: &mut HashMap<String, Entries>,
        created_folders: &mut Vec<String>,
        wrote: &mut bool,
    ) -> WriteOutcome {
        if let Err(error) = self.ensure_folders(folder, listings, created_folders, wrote) {
            return WriteOutcome::Failed(error.to_string());
        }
        if !listings.contains_key(folder) {
            match Entries::read(&self.root.join(folder)) {
                Ok(entries) => listings.insert(folder.to_string(), entries),
                Err(error) => return WriteOutcome::Failed(io_error(folder)(error).to_string()),
            };
        }
        if let Some(existing) = listings[folder].clash(name) {
            return WriteOutcome::Exists(HostId::new(join(folder, &existing)));
        }

        let file_name = format!("{name}.{HOST_EXTENSION}");
        let id = HostId::new(join(folder, &file_name));
        let blank = Host::blank(id.clone(), name);
        let mut edit = blank.clone();
        apply(&mut edit, values);
        let document = match updated_document(&blank, &edit) {
            Ok(document) => document,
            Err(error) => return WriteOutcome::Failed(error.to_string()),
        };
        match self.create_file(id.as_str(), &document) {
            Ok(()) => {}
            // Created by someone else between the listing and now.
            Err(EditError::Io { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists => {
                return WriteOutcome::Exists(id)
            }
            Err(error) => return WriteOutcome::Failed(error.to_string()),
        }
        *wrote = true;
        let entries = listings.get_mut(folder).expect("listed above");
        entries
            .names
            .insert(names::folded(&file_name), file_name.clone());
        entries.hosts.insert(names::folded(name), file_name);
        WriteOutcome::Created(id)
    }

    /// Creates the folders of `path` that are not there yet.
    fn ensure_folders(
        &self,
        path: &str,
        listings: &mut HashMap<String, Entries>,
        created_folders: &mut Vec<String>,
        wrote: &mut bool,
    ) -> Result<(), EditError> {
        if path.is_empty() || self.root.join(path).is_dir() {
            return Ok(());
        }
        let mut current = String::new();
        for name in path.split('/') {
            let next = join(&current, name);
            let dir = self.root.join(&next);
            if !dir.is_dir() {
                fs::create_dir(&dir).map_err(io_error(&next))?;
                *wrote = true;
                created_folders.push(next.clone());
                listings.insert(next.clone(), Entries::default());
                if let Some(parent) = listings.get_mut(&current) {
                    parent.names.insert(names::folded(name), name.to_string());
                }
            }
            current = next;
        }
        Ok(())
    }

    /// Writes a new file, failing if one is there already.
    fn create_file(&self, file: &str, document: &Mapping) -> Result<(), EditError> {
        let text = serde_norway::to_string(document).map_err(|error| EditError::Io {
            file: file.to_string(),
            source: io::Error::other(error),
        })?;
        let path = self.root.join(file);
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error(file))?;
        out.write_all(text.as_bytes()).map_err(|error| {
            drop(fs::remove_file(&path));
            io_error(file)(error)
        })
    }

    /// Gives the existing host `id` the imported values; returns whether its
    /// file changed.
    fn write_update(&self, id: &HostId, values: &ImportedValues) -> Result<bool, EditError> {
        let current = self.existing_host(id)?;
        let mut edit = current.clone();
        apply(&mut edit, values);
        let document = updated_document(current, &edit)?;
        if document == current.document {
            return Ok(false);
        }
        write_mapping(&self.root, id.as_str(), &document)?;
        Ok(true)
    }
}

impl Host {
    /// A host with nothing set, as a new file starts.
    fn blank(id: HostId, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            address: String::new(),
            vendor: None,
            model: None,
            serial_number: None,
            site: None,
            building: None,
            room: None,
            rack: None,
            rack_unit: None,
            role: None,
            description: None,
            tags: Vec::new(),
            custom: Vec::new(),
            settings: Settings::default(),
            document: Mapping::new(),
        }
    }

    fn text_field_mut(&mut self, key: &str) -> Option<&mut Option<String>> {
        use super::*;
        Some(match key {
            VENDOR => &mut self.vendor,
            PLATFORM => &mut self.settings.platform,
            MODEL => &mut self.model,
            SERIAL_NUMBER => &mut self.serial_number,
            SITE => &mut self.site,
            BUILDING => &mut self.building,
            ROOM => &mut self.room,
            RACK => &mut self.rack,
            RACK_UNIT => &mut self.rack_unit,
            ROLE => &mut self.role,
            DESCRIPTION => &mut self.description,
            _ => return None,
        })
    }
}

/// Sets `values` on `host`.
fn apply(host: &mut Host, values: &ImportedValues) {
    if let Some(address) = &values.address {
        host.address = address.clone();
    }
    for (key, value) in &values.text {
        if let Some(field) = host.text_field_mut(key) {
            *field = Some(value.clone());
        }
    }
    if let Some(port) = values.port {
        host.settings.port = Some(port);
    }
    if let Some(tags) = &values.tags {
        host.tags = tags.clone();
    }
    for (key, value) in &values.custom {
        let existing = host.custom.iter().position(|field| &field.key == key);
        match (existing, value.trim().is_empty()) {
            (Some(index), true) => {
                host.custom.remove(index);
            }
            (Some(index), false) => host.custom[index].value = value.clone(),
            (None, true) => {}
            (None, false) => host.custom.push(CustomField::new(key, value)),
        }
    }
}
