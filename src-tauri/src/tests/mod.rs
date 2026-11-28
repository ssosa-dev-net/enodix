//! The shell's own tests (#70): the app as `run` builds it, its state, its
//! main window and every command, on Tauri's mock runtime. No window shows
//! and no webview runs; commands are called through the same IPC the
//! interface uses, with the same arguments, and the channels it would pass
//! come back here instead of to a page.
//!
//! `app` is the harness, `sessions` opens each kind of session through
//! `session_open`, `workspace_lock` checks that nothing that locks the
//! workspace is called while it is held. The test servers are the core's
//! (`crates/core/tests/support`).

#[path = "../../../crates/core/tests/support/mod.rs"]
mod support;

mod app;
mod sessions;
mod workspace_lock;
