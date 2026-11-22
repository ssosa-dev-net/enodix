//! The desktop's opener started from an AppImage gets none of what the
//! AppImage's runtime set for the app (#58): with its `LD_LIBRARY_PATH`,
//! `GTK_PATH` and the like, the system's file manager and browser that the
//! opener starts would load the bundle's libraries and modules; and it
//! does not start in the bundle's mount, where the app runs.
//!
//! Unix only (an AppImage is), and a test binary of its own: it changes
//! this process's environment, as the AppImage's runtime does before the
//! app starts. The opener is a stand-in `xdg-open` that writes down what it
//! was given; the PATH holds nothing else that opens (under WSL, no
//! `powershell.exe`).
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use netterm_core::opener;
use netterm_core::python::BUNDLE_VARIABLE;
use tempfile::TempDir;

#[test]
fn the_opener_started_from_an_appimage_gets_none_of_the_bundles_variables() {
    let folder = TempDir::new().unwrap();
    let bundle = folder.path().join(".mount_netterm");
    let inside = |path: &str| bundle.join(path);
    let joined = |paths: &[PathBuf]| std::env::join_paths(paths).unwrap();
    let bin = folder.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let written = folder.path().join("opened.txt");
    let opener_file = bin.join("xdg-open");
    fs::write(
        &opener_file,
        "#!/bin/sh\n{ echo \"target=$1\"; echo \"cwd=$(pwd)\"; env; } > \"$OPENED_TO.part\"\nmv \"$OPENED_TO.part\" \"$OPENED_TO\"\n",
    )
    .unwrap();
    fs::set_permissions(&opener_file, fs::Permissions::from_mode(0o755)).unwrap();

    let home = folder.path().join("home");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(inside("usr/bin")).unwrap();

    assert!(!opener::in_a_bundle());
    // As the AppImage's runtime leaves it: the app in the bundle's mount,
    // the runtime's own variables set.
    std::env::set_current_dir(&bundle).unwrap();
    std::env::set_var(BUNDLE_VARIABLE, &bundle);
    std::env::set_var("APPIMAGE", folder.path().join("netterm.AppImage"));
    std::env::set_var("ARGV0", "./netterm.AppImage");
    std::env::set_var("OWD", folder.path());
    std::env::set_var("HOME", &home);
    std::env::set_var(
        "LD_LIBRARY_PATH",
        joined(&[inside("usr/lib"), PathBuf::from("/opt/keep")]),
    );
    std::env::set_var("GTK_PATH", inside("usr/lib/gtk-3.0"));
    std::env::set_var("GDK_PIXBUF_MODULE_FILE", inside("usr/lib/loaders.cache"));
    std::env::set_var(
        "PATH",
        joined(&[
            bin.clone(),
            inside("usr/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]),
    );
    std::env::set_var("OPENED_TO", &written);
    assert!(opener::in_a_bundle());

    opener::open_detached("/home/someone/logs/r1").unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while !written.exists() {
        assert!(Instant::now() < deadline, "the opener did not run");
        std::thread::sleep(Duration::from_millis(20));
    }
    let given = fs::read_to_string(&written).unwrap();
    let lines: Vec<&str> = given.lines().collect();
    assert!(lines.contains(&"target=/home/someone/logs/r1"), "{given}");
    assert!(lines.contains(&"LD_LIBRARY_PATH=/opt/keep"), "{given}");
    assert!(
        !given.contains(".mount_netterm"),
        "something points into the bundle: {given}"
    );
    for name in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name} was passed on: {given}"
        );
    }
    // It starts in the home folder, not in the bundle's mount.
    let cwd = format!("cwd={}", home.display());
    assert!(lines.contains(&cwd.as_str()), "{given}");
    let path = lines
        .iter()
        .find_map(|line| line.strip_prefix("PATH="))
        .unwrap_or_default();
    assert_eq!(
        path,
        format!("{}:/usr/bin:/bin", bin.display()),
        "the PATH keeps its other entries"
    );
}
