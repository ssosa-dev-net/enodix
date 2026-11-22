//! Handing a folder or a web address to the desktop: its opener
//! (`xdg-open` and the others the `open` crate knows, in its order) starts
//! the file manager or the browser.
//!
//! Started from an AppImage, the app's environment points into the
//! bundle (`LD_LIBRARY_PATH`, `GTK_PATH`, `GIO_MODULE_DIR`,
//! `GDK_PIXBUF_MODULE_FILE`, `PYTHONHOME`, ...), and the `open` crate
//! passes all of it on: the system's file manager and browser then load
//! the bundle's libraries and modules instead of their own (#58). Here the
//! opener gets none of it ([`crate::python::leave_the_bundle`]); otherwise
//! it is what `open::that_detached` does.

use std::ffi::OsStr;
use std::io;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::python::{leave_the_bundle, BUNDLE_VARIABLE};

/// How long a started opener is watched for failing at once (as the
/// `open` crate does); one still running then has taken the target over.
const FAILURE_CHECK: Duration = Duration::from_millis(250);

/// Whether this process runs from a bundle (an AppImage), whose variables
/// the programs it starts must not get.
pub fn in_a_bundle() -> bool {
    std::env::var_os(BUNDLE_VARIABLE).is_some_and(|folder| !folder.is_empty())
}

/// Opens `target` (a folder, a file, a web address) with the desktop's
/// opener, detached; the opener gets none of a bundle's variables. Each
/// opener is tried in turn until one starts and does not fail at once.
pub fn open_detached(target: impl AsRef<OsStr>) -> io::Result<()> {
    let mut first_failure = None;
    let mut last_error = None;
    let bundled = in_a_bundle();
    for mut command in open::commands(target) {
        leave_the_bundle(&mut command);
        if bundled {
            // What the runtime says about itself, and the folder the app
            // runs in (inside the bundle's mount): the file manager or
            // browser would hold the mount busy, and stand in a folder
            // that is gone once the app ends.
            for name in [BUNDLE_VARIABLE, "APPIMAGE", "ARGV0", "OWD"] {
                command.env_remove(name);
            }
            let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
            command.current_dir(home.as_deref().unwrap_or(OsStr::new("/")));
        }
        match launch(&mut command) {
            Ok(None) => return Ok(()),
            Ok(Some(failure)) => {
                first_failure.get_or_insert(failure);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(first_failure
        .or(last_error)
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no opener was found")))
}

/// Starts `command` on its own (no terminal, no input or output); `None`
/// once it runs on or has succeeded, else why it failed at once.
fn launch(command: &mut Command) -> io::Result<Option<io::Error>> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and touches no memory of
        // the parent.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((!status.success()).then(|| {
                io::Error::other(format!(
                    "{} failed ({status})",
                    command.get_program().to_string_lossy()
                ))
            }));
        }
        if started.elapsed() >= FAILURE_CHECK {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    // Still running: it took the target over. Reaped when it ends.
    wait_in_the_background(child);
    Ok(None)
}

fn wait_in_the_background(mut child: Child) {
    let _ = thread::Builder::new()
        .name("opener wait".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let _ = child.wait();
        });
}
