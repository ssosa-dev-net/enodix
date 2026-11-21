//! The process's memory and handles, for tests that check nothing grows
//! without bound and nothing is left behind (the transfer engine's, #105;
//! sessions under a flood, #118). Such tests live in files of their own, or
//! hold [`exclusive`] while they measure, so that no other test shares the
//! process meanwhile.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const MIB: u64 = 1024 * 1024;

/// Held while a test measures the process, so that the other tests of its
/// file wait.
pub async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    ONE.lock().await
}

/// The process's resident memory, in bytes, where the platform tells.
#[cfg(windows)]
pub fn resident() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the counters are ours and sized as the call is told.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    (ok != 0).then_some(counters.WorkingSetSize as u64)
}

#[cfg(target_os = "linux")]
pub fn resident() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(pages * page as u64)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn resident() -> Option<u64> {
    None
}

/// How many handles (Windows) or file descriptors (Linux) the process has
/// open, where the platform tells: sockets and files left open show here.
#[cfg(windows)]
pub fn open_handles() -> Option<u64> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut count = 0u32;
    // SAFETY: the count is ours.
    let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    (ok != 0).then_some(u64::from(count))
}

#[cfg(target_os = "linux")]
pub fn open_handles() -> Option<u64> {
    Some(std::fs::read_dir("/proc/self/fd").ok()?.count() as u64)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn open_handles() -> Option<u64> {
    None
}

/// Samples the resident memory until stopped; the highest seen.
pub struct Peak {
    highest: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Peak {
    pub fn start() -> Self {
        let highest = Arc::new(AtomicU64::new(resident().unwrap_or(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (highest, stop) = (highest.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    if let Some(now) = resident() {
                        highest.fetch_max(now, Ordering::SeqCst);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        Self {
            highest,
            stop,
            thread,
        }
    }

    pub fn stop(self) -> u64 {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap();
        self.highest.load(Ordering::SeqCst)
    }
}
