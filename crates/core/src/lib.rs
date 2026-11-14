//! The UI-free core of netterm.
//!
//! All behaviour lives in this crate. The Tauri shell is a thin adapter over
//! its public API and the interface only renders; that public API is also the
//! single test seam (see `docs/spec-v1.md`, "Architecture" and "Testing
//! Decisions").

pub mod app_state;
pub mod credentials;
pub mod known_hosts;
pub mod layout;
pub mod session;
pub mod workspace;
