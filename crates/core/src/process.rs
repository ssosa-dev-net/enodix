//! The programs the core starts (a script job's interpreter, the Python
//! setup's venv and pip, the interpreter check), held together with every
//! process they start in turn, so that they end together: when cancelled
//! ([`Tree::stop`]), when the program has ended and its [`Tree`] is
//! dropped (what it left behind goes too), and when this process ends in
//! any way at all, a crash or a kill included, with no exit hook to run.
//!
//! - On Windows the program starts suspended, is put in a job object of
//!   its own that ends its processes when its last handle closes
//!   (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), and only then runs, so that
//!   nothing it starts is outside the job (a process cannot leave it: the
//!   job allows no breakaway). [`Tree::stop`] ends the job's processes;
//!   the only handle is the [`Tree`]'s, which the system closes when this
//!   process ends however it ends.
//! - Elsewhere the program leads a session and process group of its own
//!   ([`crate::python::quiet_command`]), and a keeper forked into that
//!   group before the program runs waits on a pipe whose only writing end
//!   is the [`Tree`]'s: once that closes (dropped, or this process ended
//!   however it ended), the keeper ends its process group, itself
//!   included. Being in the group, it also keeps the group's id from
//!   naming another group meanwhile. [`Tree::stop`] ends the group.
//!
//! Not covered (the notes say so too): on Windows, a program that has
//! something else start a process for it (a service, the task scheduler,
//! an application that was already running); elsewhere, a process that
//! leaves the group (`setsid`, `setpgid`, a daemon), and the keeper being
//! killed on its own.

use std::io;
use std::process::{Child, Command};

/// `CREATE_NO_WINDOW`: no console window flashes up.
#[cfg(windows)]
pub(crate) const NO_WINDOW: u32 = 0x0800_0000;

/// A started program and the processes it starts: see the module.
pub(crate) struct Tree {
    pub(crate) child: Child,
    /// The job object holding them all (`None`: it could not be made,
    /// and [`Tree::stop`] falls back to ending the process tree).
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
    /// The writing end of the keeper's pipe (`None`: no keeper, as the
    /// program leads no process group).
    #[cfg(unix)]
    _keeper: Option<io::PipeWriter>,
}

impl Tree {
    /// Ends the program and the processes it started, and waits for the
    /// program. Call it before the program has been waited for: until
    /// then its id cannot name anything else.
    pub(crate) fn stop(&mut self) {
        #[cfg(windows)]
        windows::stop(self.job.as_ref(), &self.child);
        #[cfg(unix)]
        if let Ok(group) = libc::pid_t::try_from(self.child.id()) {
            // SAFETY: a plain system call; a negative id names the group
            // the program leads (it and its keeper keep that id theirs).
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `command` (made by [`crate::python::quiet_command`]) as a
/// [`Tree`]. On Unix, a program written a moment ago can be refused as
/// "text file busy" while another thread of this process is starting a
/// program of its own (its child briefly holds the file open for writing,
/// until it runs its own program): that passes in a moment, so it is tried
/// again for a while.
pub(crate) fn start(command: &mut Command) -> io::Result<Tree> {
    #[cfg(windows)]
    {
        windows::start(command)
    }
    #[cfg(unix)]
    {
        unix::start(command)
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
    };

    use super::{Tree, NO_WINDOW};

    pub(super) fn start(command: &mut Command) -> io::Result<Tree> {
        // Suspended until it is in the job, so that nothing it starts
        // escapes the job.
        command.creation_flags(NO_WINDOW | CREATE_SUSPENDED);
        let mut child = command.spawn()?;
        let job = new_job().and_then(|job| {
            // SAFETY: both handles are open (the child's until it is
            // waited for, which has not happened).
            let assigned =
                unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) };
            if assigned == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(job)
            }
        });
        if let Err(error) = &job {
            eprintln!(
                "a started program is not held with the processes it starts ({error}): it may outlive the app"
            );
        }
        if let Err(error) = resume(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Tree {
            child,
            job: job.ok(),
        })
    }

    /// A job object that ends its processes when its last handle closes.
    fn new_job() -> io::Result<OwnedHandle> {
        // SAFETY: no name and no security attributes; the handle is owned
        // at once.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle that nothing else owns.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: plain data, all zero is its empty value.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the structure and its size match the class asked for.
        let set = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Lets the suspended `child` run: resumes its one thread.
    fn resume(child: &Child) -> io::Result<()> {
        let pid = child.id();
        // SAFETY: a snapshot of every thread, owned at once.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle that nothing else owns.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: plain data; its size is set as the call requires.
        let mut entry: THREADENTRY32 = unsafe { mem::zeroed() };
        entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;
        // SAFETY: the snapshot is open and `entry` is sized.
        let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: a thread of the child; its handle is closed here.
                unsafe {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        resumed |= ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                }
            }
            // SAFETY: as for the first.
            more = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other("the started program could not be let run"))
        }
    }

    pub(super) fn stop(job: Option<&OwnedHandle>, child: &Child) {
        match job {
            // SAFETY: the job's handle is open.
            Some(job) => unsafe {
                TerminateJobObject(job.as_raw_handle(), 1);
            },
            // Without a job: the process tree as it is now.
            None => {
                let _ = Command::new("taskkill")
                    .creation_flags(NO_WINDOW)
                    .args(["/T", "/F", "/PID", &child.id().to_string()])
                    .output();
            }
        }
    }
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::Tree;

    /// The highest descriptor the keeper closes when it cannot close them
    /// all at once.
    const MOST_DESCRIPTORS: libc::c_int = 65_536;

    pub(super) fn start(command: &mut Command) -> io::Result<Tree> {
        let (reader, writer) = io::pipe()?;
        let read_end = reader.as_raw_fd();
        let highest = highest_descriptor();
        // SAFETY: what runs between fork and exec only makes system calls
        // that are async-signal-safe and touches no memory of the parent.
        unsafe {
            command.pre_exec(move || {
                // Only a program that leads its own group gets a keeper:
                // ending the keeper's group must never end this process.
                if libc::getpgrp() != libc::getpid() {
                    return Ok(());
                }
                match libc::fork() {
                    -1 => Err(io::Error::last_os_error()),
                    0 => keep(read_end, highest),
                    _ => Ok(()),
                }
            });
        }
        let until = Instant::now() + Duration::from_secs(2);
        let child = loop {
            match command.spawn() {
                Err(error)
                    if error.kind() == io::ErrorKind::ExecutableFileBusy
                        && Instant::now() < until =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                result => break result?,
            }
        };
        // The keeper holds the reading end; this process only the writing
        // one (the program closed both when it started: they are
        // close-on-exec).
        drop(reader);
        Ok(Tree {
            child,
            _keeper: Some(writer),
        })
    }

    /// The keeper, in the program's process group: waits until the
    /// writing end of its pipe is closed everywhere, then ends the group.
    fn keep(read_end: libc::c_int, highest: libc::c_int) -> ! {
        // SAFETY: async-signal-safe calls only, in a child of fork.
        unsafe {
            // No handler of the parent's ever runs here.
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::sigprocmask(libc::SIG_SETMASK, &all, std::ptr::null_mut());
            // Keep only the pipe: the program's output pipes and the
            // parent's descriptors are not held open by the keeper.
            libc::dup2(read_end, 0);
            close_from(1, highest);
            let mut byte = 0u8;
            loop {
                let read = libc::read(0, std::ptr::from_mut(&mut byte).cast(), 1);
                if read > 0
                    || (read < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted)
                {
                    continue;
                }
                break;
            }
            libc::kill(0, libc::SIGKILL);
            libc::_exit(0)
        }
    }

    /// Closes every descriptor from `first` on.
    ///
    /// # Safety
    /// Only between fork and exec, or in a forked child.
    unsafe fn close_from(first: libc::c_int, highest: libc::c_int) {
        #[cfg(target_os = "linux")]
        // SAFETY: closes descriptors only.
        if unsafe { libc::syscall(libc::SYS_close_range, first as u32, u32::MAX, 0u32) } == 0 {
            return;
        }
        for descriptor in first..=highest {
            // SAFETY: closing a descriptor that may not be open is harmless.
            unsafe {
                libc::close(descriptor);
            }
        }
    }

    /// The highest descriptor this process may have open, as far as the
    /// keeper needs to close them one by one.
    fn highest_descriptor() -> libc::c_int {
        // SAFETY: plain data filled by the call.
        let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
            return MOST_DESCRIPTORS;
        }
        libc::c_int::try_from(limit.rlim_cur)
            .unwrap_or(MOST_DESCRIPTORS)
            .min(MOST_DESCRIPTORS)
    }
}
