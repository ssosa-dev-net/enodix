//! Programs the app starts do not inherit what an AppImage's runtime set
//! for the app (#114): with its `PYTHONHOME` and `PYTHONPATH` the system's
//! Python could not even start ("No module named 'encodings'"), so the
//! AppImage found no Python.
//!
//! A test binary of its own: it changes this process's environment, as
//! the AppImage's runtime does before the app starts.

mod support;

use std::path::PathBuf;

use enodix_core::python::{self, BUNDLE_VARIABLE};
use support::python::FakePython;
use tempfile::TempDir;

#[test]
fn a_python_started_from_an_appimage_gets_none_of_the_bundles_variables() {
    let folder = TempDir::new().unwrap();
    let bundle = folder.path().join(".mount_enodix");
    let inside = |path: &str| bundle.join(path);
    let joined = |paths: &[PathBuf]| std::env::join_paths(paths).unwrap();
    std::env::set_var(BUNDLE_VARIABLE, &bundle);
    std::env::set_var("PYTHONHOME", inside("usr"));
    std::env::set_var("PYTHONPATH", joined(&[inside("usr/share/pyshared")]));
    std::env::set_var(
        "LD_LIBRARY_PATH",
        joined(&[inside("usr/lib"), PathBuf::from("/opt/keep")]),
    );
    std::env::set_var("ENODIX_KEEP_ME", "kept");

    // It answers as Python only with none of the bundle in its environment.
    let answer = serde_json::json!({
        "probe": 1,
        "version": [3, 12, 0],
        "executable": "/usr/bin/python3",
        "package": null,
        "package_error": "ModuleNotFoundError: No module named enodix",
    });
    let python = FakePython::running(
        folder.path(),
        "python3",
        &format!(
            "if defined PYTHONHOME (echo leaked PYTHONHOME & exit /b 1)\r\n\
             if defined PYTHONPATH (echo leaked PYTHONPATH & exit /b 1)\r\n\
             if not \"%LD_LIBRARY_PATH%\"==\"/opt/keep\" (echo LD_LIBRARY_PATH=%LD_LIBRARY_PATH% & exit /b 1)\r\n\
             if not \"%ENODIX_KEEP_ME%\"==\"kept\" (echo lost ENODIX_KEEP_ME & exit /b 1)\r\n\
             echo {answer}"
        ),
        &format!(
            "[ -n \"${{PYTHONHOME+set}}\" ] && echo leaked PYTHONHOME && exit 1\n\
             [ -n \"${{PYTHONPATH+set}}\" ] && echo leaked PYTHONPATH && exit 1\n\
             [ \"$LD_LIBRARY_PATH\" != /opt/keep ] && echo LD_LIBRARY_PATH=$LD_LIBRARY_PATH && exit 1\n\
             [ \"$ENODIX_KEEP_ME\" != kept ] && echo lost ENODIX_KEEP_ME && exit 1\n\
             echo '{answer}'"
        ),
    );

    let found = python::probe(&python.candidate());

    let interpreter = found.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(interpreter.executable, PathBuf::from("/usr/bin/python3"));
}
