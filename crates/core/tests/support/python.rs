//! Stand-ins for Python interpreters, and the real one when there is one.
//!
//! A [`FakePython`] is a small script (a `.cmd` file on Windows, a shell
//! script elsewhere) that answers the interpreter check the way a Python
//! of the given version would, with or without the app's package. It
//! ignores its arguments, so it can also stand in as the base Python of a
//! setup (it "creates" nothing). [`FakePython::failing`] prints a line and
//! fails; [`FakePython::sleeping`] does not end for a minute.
//!
//! [`real_python`] is a Python 3.10+ of this machine (`NETTERM_TEST_PYTHON`,
//! else where the app looks), for the tests that run only when one is
//! present; [`build_wheel`] builds the app's wheel with
//! `scripts/build-wheel.mjs` (needs Node).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use netterm_core::python::{self, Candidate, Interpreter, PythonVersion};

pub struct FakePython {
    pub path: PathBuf,
}

impl FakePython {
    /// A Python `version` (`"3.9.1"`) that imports the package in version
    /// `package` (`None`: it cannot import it).
    pub fn answering(folder: &Path, name: &str, version: &str, package: Option<&str>) -> Self {
        let path = script_path(folder, name);
        let parts: Vec<u32> = version
            .split('.')
            .map(|part| part.parse().unwrap())
            .collect();
        let answer = serde_json::json!({
            "probe": 1,
            "version": parts,
            "executable": path,
            "package": package,
            "package_error": package.is_none().then_some("ModuleNotFoundError: No module named netterm"),
        });
        write_script(
            &path,
            &format!("echo {answer}"),
            &format!("echo '{answer}'"),
        );
        Self { path }
    }

    /// A program that prints a line and fails with exit status 3.
    pub fn failing(folder: &Path, name: &str) -> Self {
        let path = script_path(folder, name);
        write_script(
            &path,
            "echo this is not python\r\nexit /b 3",
            "echo this is not python\nexit 3",
        );
        Self { path }
    }

    /// A program that does not end for a minute.
    pub fn sleeping(folder: &Path, name: &str) -> Self {
        let path = script_path(folder, name);
        write_script(&path, "ping -n 60 127.0.0.1 > nul", "sleep 60");
        Self { path }
    }

    pub fn candidate(&self) -> Candidate {
        Candidate::new(&self.path)
    }
}

fn script_path(folder: &Path, name: &str) -> PathBuf {
    folder.join(if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_string()
    })
}

fn write_script(path: &Path, windows: &str, unix: &str) {
    if cfg!(windows) {
        fs::write(path, format!("@echo off\r\n{windows}\r\n")).unwrap();
    } else {
        fs::write(path, format!("#!/bin/sh\n{unix}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

/// A Python 3.10 or newer of this machine, or `None` (the caller skips,
/// saying so).
pub fn real_python() -> Option<Interpreter> {
    let mut candidates = Vec::new();
    if let Some(chosen) = std::env::var_os("NETTERM_TEST_PYTHON") {
        candidates.push(Candidate::new(chosen));
    }
    candidates.extend(python::system_candidates());
    python::find_python(&candidates).ok()
}

pub fn too_old() -> PythonVersion {
    PythonVersion {
        major: 3,
        minor: 9,
        patch: 1,
    }
}

/// Builds the app's wheel into `folder` with the app's build script; `None`
/// when Node cannot run it.
pub fn build_wheel(folder: &Path) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let done = Command::new("node")
        .arg(root.join("scripts/build-wheel.mjs"))
        .arg(folder)
        .output()
        .ok()?;
    if !done.status.success() {
        panic!(
            "build-wheel.mjs failed: {}",
            String::from_utf8_lossy(&done.stderr)
        );
    }
    let wheel = folder.join(python::wheel_name(env!("CARGO_PKG_VERSION")));
    assert!(wheel.is_file(), "no wheel at {}", wheel.display());
    Some(wheel)
}
