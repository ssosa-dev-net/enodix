//! Named secrets: a name and a text value per workspace (credential
//! scope), for scripts and integrations. Stores keep them apart from the
//! credential profiles, so a profile and a secret can share a name.
//!
//! Listing gives names only. [`Secrets::get`] reads a value for the
//! consumers that need one (the script bridge, integrations); the
//! interface never calls it. `Debug` of a [`SecretValue`] hides it.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{CredentialStore, StoreError};
use crate::workspace::Workspace;

/// The longest secret name accepted, in characters.
pub const MAX_SECRET_NAME: usize = 100;

/// A secret's value. Its `Debug` output hides it; read it with
/// [`Self::expose`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("there is no secret called {0}")]
    NotFound(String),
    #[error("a secret called {0} already exists")]
    AlreadyExists(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the credential store failed: {0}")]
    Store(#[from] StoreError),
}

/// The named secrets of one workspace.
pub struct Secrets<'a> {
    store: &'a dyn CredentialStore,
    scope: String,
}

impl<'a> Secrets<'a> {
    pub fn new(store: &'a dyn CredentialStore, workspace: &Workspace) -> Self {
        Self {
            store,
            scope: workspace.credential_scope(),
        }
    }

    /// The names of the stored secrets, by name ignoring case. Never the
    /// values.
    pub fn list(&self) -> Result<Vec<String>, SecretError> {
        let mut names = self.store.secret_names(&self.scope)?;
        names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
        Ok(names)
    }

    /// The value of the secret called `name` (ignoring case), if there is
    /// one. For consumers that use the value; nothing shows it.
    pub fn get(&self, name: &str) -> Result<Option<SecretValue>, SecretError> {
        Ok(self.store.get_secret(&self.scope, name.trim())?)
    }

    /// Keeps a new secret; refused if one of that name (ignoring case)
    /// exists. Returns the name as stored (trimmed).
    pub fn add(&self, name: &str, value: &str) -> Result<String, SecretError> {
        let name = name.trim();
        validate_name(name)?;
        validate_value(value)?;
        if self.store.get_secret(&self.scope, name)?.is_some() {
            return Err(SecretError::AlreadyExists(name.into()));
        }
        self.store
            .put_secret(&self.scope, name, &SecretValue::new(value))?;
        Ok(name.into())
    }

    /// Replaces the value of the secret called `name` (ignoring case),
    /// keeping its name as first written.
    pub fn replace(&self, name: &str, value: &str) -> Result<(), SecretError> {
        let name = name.trim();
        validate_value(value)?;
        let stored = self
            .list()?
            .into_iter()
            .find(|stored| stored.to_lowercase() == name.to_lowercase())
            .ok_or_else(|| SecretError::NotFound(name.into()))?;
        self.store
            .put_secret(&self.scope, &stored, &SecretValue::new(value))?;
        Ok(())
    }

    /// Deletes the secret called `name` (ignoring case).
    pub fn delete(&self, name: &str) -> Result<(), SecretError> {
        let name = name.trim();
        if self.store.delete_secret(&self.scope, name)? {
            Ok(())
        } else {
            Err(SecretError::NotFound(name.into()))
        }
    }
}

/// Names: letters, digits, `-`, `_` and `.` (ASCII), at most
/// [`MAX_SECRET_NAME`].
fn validate_name(name: &str) -> Result<(), SecretError> {
    if name.is_empty() {
        return Err(SecretError::Invalid("a secret needs a name".into()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(SecretError::Invalid(
            "a secret name can only hold letters, digits, -, _ and .".into(),
        ));
    }
    if name.chars().count() > MAX_SECRET_NAME {
        return Err(SecretError::Invalid(format!(
            "a secret name can have at most {MAX_SECRET_NAME} characters"
        )));
    }
    Ok(())
}

fn validate_value(value: &str) -> Result<(), SecretError> {
    if value.is_empty() {
        return Err(SecretError::Invalid("a secret needs a value".into()));
    }
    Ok(())
}
