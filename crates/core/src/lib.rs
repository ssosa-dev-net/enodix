//! The UI-free core of netterm.
//!
//! All behaviour lives in this crate. The Tauri shell is a thin adapter over
//! its public API and the interface only renders; that public API is also the
//! single test seam (see `docs/spec-v1.md`, "Architecture" and "Testing
//! Decisions").

pub mod app_state;
pub mod configs;
pub mod credentials;
pub mod diff;
pub mod enrich;
pub mod files;
pub mod highlight;
pub mod history;
pub mod import;
pub mod inventory;
pub mod job;
pub mod known_hosts;
pub mod layout;
pub mod logging;
mod names;
pub mod paste;
pub mod platform;
pub mod preferences;
mod process;
pub mod python;
pub mod session;
pub mod template;
pub mod tunnel;
pub mod update;
pub mod workspace;
