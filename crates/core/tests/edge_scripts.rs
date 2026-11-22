//! Edge testing of script jobs (#119): a script that floods both streams,
//! prints bytes that are not UTF-8, reads its standard input, ignores the
//! request to stop, holds a connection open, ends in every way; cancelling
//! each; a package the script needs missing. The scripts are this test
//! binary run again in a role (`support::processes`), behind a stand-in
//! interpreter, so that they behave the same on Windows and Linux; one
//! test uses a real Python and is SKIPPED without one.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use enodix_core::job::results::{OutputRef, RunStore};
use enodix_core::job::{
    self, output_cut, HostState, JobEvent, JobOptions, JobStep, ScriptOptions, SCRIPT_OUTPUT,
};
use enodix_core::workspace::HostId;
use support::jobs::{folder, follow, Lab, Ran};
use support::memory::{self, MIB};
use support::processes::{self, PIDS_VARIABLE, ROLE_VARIABLE};
use support::python::{real_python, FakePython};

/// How much a flood writes to each stream, and the exit status to end with.
const SIZE_VARIABLE: &str = "ENODIX_TEST_SIZE";
const CODE_VARIABLE: &str = "ENODIX_TEST_CODE";
const PORT_VARIABLE: &str = "ENODIX_TEST_PORT";

const FLOOD: &str = "flood";
const NOT_UTF8: &str = "not utf8";
const STDIN: &str = "stdin";
const EXIT: &str = "exit";
const ABORT: &str = "abort";
const PANIC: &str = "panic";
const STUBBORN: &str = "stubborn";
const CONNECT: &str = "connect";

/// This binary run again as a script: [`processes::play`]'s roles, and this
/// file's.
#[test]
fn helper() {
    processes::play();
    let Some(role) = processes::role() else {
        return;
    };
    if let Some(pids) = std::env::var_os(PIDS_VARIABLE) {
        processes::write_pid(Path::new(&pids), processes::CHILD);
    }
    let number = |name: &str| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    match role.as_str() {
        FLOOD => {
            // Both streams in turn, a line at a time in large writes.
            let size = number(SIZE_VARIABLE);
            let line = b"flood flood flood flood flood flood flood flood flood flood flood\n";
            let chunk = line.repeat(64 * 1024 / line.len());
            let mut out = std::io::stdout().lock();
            let mut err = std::io::stderr().lock();
            let mut written = 0;
            while written < size {
                if out.write_all(&chunk).is_err() || err.write_all(&chunk).is_err() {
                    std::process::exit(9);
                }
                written += chunk.len() as u64;
            }
            let _ = out.write_all(b"flood done\n");
            let _ = out.flush();
            std::process::exit(0)
        }
        NOT_UTF8 => {
            let mut out = std::io::stdout().lock();
            out.write_all(b"latin-1: caf\xe9\n").unwrap();
            out.write_all(b"broken: \xff\xfe\xc3\n").unwrap();
            // A character split across two writes, a moment apart.
            out.write_all(b"split: \xe2\x82").unwrap();
            out.flush().unwrap();
            std::thread::sleep(Duration::from_millis(200));
            out.write_all(b"\xac euro\n").unwrap();
            out.flush().unwrap();
            std::process::exit(0)
        }
        STDIN => {
            let mut read = Vec::new();
            let got = std::io::stdin().read_to_end(&mut read);
            println!("stdin gave {} bytes ({})", read.len(), got.is_ok());
            std::process::exit(0)
        }
        EXIT => std::process::exit(number(CODE_VARIABLE) as i32),
        ABORT => std::process::abort(),
        PANIC => panic!("the script panicked"),
        STUBBORN => {
            ignore_stop_requests();
            println!("stubborn: ignoring requests to stop");
            std::thread::sleep(Duration::from_secs(60));
            std::process::exit(0)
        }
        CONNECT => {
            let port = number(PORT_VARIABLE) as u16;
            let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            socket.write_all(b"hello").unwrap();
            println!("connected");
            std::thread::sleep(Duration::from_secs(60));
            std::process::exit(0)
        }
        _ => {}
    }
}

/// Ignores Ctrl+C and Ctrl+Break (Windows), and SIGINT, SIGTERM and SIGHUP
/// (Unix): what a script that catches them all would do.
#[cfg(windows)]
fn ignore_stop_requests() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    // SAFETY: no handler, TRUE: the process ignores Ctrl+C.
    unsafe { SetConsoleCtrlHandler(None, 1) };
}

#[cfg(unix)]
fn ignore_stop_requests() {
    // SAFETY: setting signals to be ignored.
    unsafe {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(signal, libc::SIG_IGN);
        }
    }
}

/// A workspace with one host and a script (never read: the stand-in runs
/// a role).
fn lab() -> Lab {
    Lab::new(&[
        ("r1.yaml", "address: 10.0.0.1\n".into()),
        ("check.py", "print('stand-in')\n".into()),
    ])
}

fn step() -> JobStep {
    JobStep::Script {
        path: "check.py".into(),
    }
}

/// A stand-in interpreter that runs this binary in the role its
/// environment names (on Unix in its place, so that a signal it dies of is
/// the script's).
fn stand_in(lab: &Lab) -> FakePython {
    let line = processes::tree_line();
    FakePython::running(
        &lab.folder("python"),
        "python",
        &line,
        &format!("exec {line}"),
    )
}

/// Script options running `role` with `extra` variables, its pid written to
/// `pids`.
fn options(python: &FakePython, role: &str, pids: &Path, extra: &[(&str, String)]) -> JobOptions {
    let mut environment = vec![
        (ROLE_VARIABLE.into(), role.into()),
        (PIDS_VARIABLE.into(), pids.as_os_str().to_owned()),
    ];
    for (name, value) in extra {
        environment.push((name.into(), value.into()));
    }
    JobOptions {
        script: Some(ScriptOptions {
            interpreter: python.path.clone(),
            cli: None,
            environment,
        }),
        ..JobOptions::default()
    }
}

/// A folder for a role's pid.
fn pids(lab: &Lab, name: &str) -> PathBuf {
    lab.folder(&format!("pids-{name}"))
}

/// The script's pid, once written.
fn pid(pids: &Path) -> u32 {
    processes::wait_for_pids(pids, &[processes::CHILD], Duration::from_secs(60))
        .expect("the script started")[0]
}

fn assert_gone(pid: u32, what: &str) {
    let alive = processes::wait_gone(&[pid], Duration::from_secs(30));
    assert!(alive.is_empty(), "{what}: {pid} still runs");
}

/// Runs `role` to its end.
async fn run_role(lab: &Lab, role: &str, extra: &[(&str, String)]) -> Ran {
    let python = stand_in(lab);
    let pids = pids(lab, &role.replace(' ', "-"));
    let plan = lab.plan(&folder(""), &step());
    let job = job::start(
        &support::manager(),
        plan,
        options(&python, role, &pids, extra),
    )
    .unwrap();
    follow(job, Duration::from_secs(120), |_, _| {}).await
}

fn script(ran: &Ran) -> &job::HostResult {
    ran.result(SCRIPT_OUTPUT)
}

/// A script flooding both streams: what is kept stops at the output limit
/// and says so (in the result, the events and the kept run), memory stays
/// far below the flood, and the script still runs to its end (its output is
/// read, not left to block it).
#[tokio::test]
async fn a_flood_on_both_streams_is_kept_up_to_the_limit_and_says_so() {
    let _alone = memory::exclusive().await;
    let lab = lab();
    let python = stand_in(&lab);
    let pids = pids(&lab, "flood");
    let limit = MIB as usize;
    let each = 32 * MIB;
    let plan = lab.plan(&folder(""), &step());
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
    let id = recorder.id().to_string();
    let options = JobOptions {
        output_limit: limit,
        ..options(&python, FLOOD, &pids, &[(SIZE_VARIABLE, each.to_string())])
    };
    let before = memory::resident();
    let peak = memory::Peak::start();
    let started = Instant::now();
    let job = job::start(&support::manager(), plan, options).unwrap();
    let ran = follow(job, Duration::from_secs(120), |_, event| {
        recorder.record(event).unwrap()
    })
    .await;
    let took = started.elapsed();
    let highest = peak.stop();
    let result = script(&ran);
    assert_eq!(result.state, HostState::Done, "{}", &result.output[..200]);
    let cut = output_cut(limit);
    assert!(result.output.ends_with(&cut), "the cut is said");
    assert!(result.output.len() <= limit + cut.len() + 1);
    assert!(
        !result.output.contains("flood done"),
        "nothing after the cut"
    );
    let kept = store
        .output(
            &ws,
            &OutputRef {
                run: id,
                host: HostId::new(SCRIPT_OUTPUT),
            },
        )
        .unwrap();
    assert_eq!(kept, result.output);
    let grew = highest.saturating_sub(before.unwrap_or(highest));
    println!(
        "a flood of 2 x {} MiB kept to {} MiB in {took:?}; memory peak +{} MiB",
        each / MIB,
        limit / MIB as usize,
        grew / MIB
    );
    if before.is_some() {
        assert!(grew < 32 * MIB, "memory grew {} MiB", grew / MIB);
    }
    assert_gone(pid(&pids), "the flood after its end");
}

/// The ticket's size: hundreds of megabytes on both streams, with the
/// default limit.
#[tokio::test]
#[ignore = "heavy: 2 x 256 MiB through a pipe (about a minute in a debug build); `cargo test -p enodix-core --test edge_scripts -- --ignored --nocapture`"]
async fn a_flood_of_hundreds_of_megabytes_stays_bounded() {
    let _alone = memory::exclusive().await;
    let lab = lab();
    let before = memory::resident();
    let peak = memory::Peak::start();
    let started = Instant::now();
    let ran = run_role(&lab, FLOOD, &[(SIZE_VARIABLE, (256 * MIB).to_string())]).await;
    let took = started.elapsed();
    let highest = peak.stop();
    let result = script(&ran);
    assert_eq!(result.state, HostState::Done);
    assert!(result
        .output
        .ends_with(&output_cut(job::DEFAULT_OUTPUT_LIMIT)));
    let grew = highest.saturating_sub(before.unwrap_or(highest));
    println!(
        "a flood of 2 x 256 MiB in {took:?}; memory peak +{} MiB",
        grew / MIB
    );
    // The kept output (32 MiB), the events holding it once more, and room.
    if before.is_some() {
        assert!(grew < 160 * MIB, "memory grew {} MiB", grew / MIB);
    }
}

/// Cancelling a flood ends it at once, its process gone.
#[tokio::test]
async fn cancelling_a_flood_ends_it() {
    let lab = lab();
    let python = stand_in(&lab);
    let pids = pids(&lab, "cancel-flood");
    let plan = lab.plan(&folder(""), &step());
    let options = JobOptions {
        output_limit: 64 * 1024,
        ..options(
            &python,
            FLOOD,
            &pids,
            &[(SIZE_VARIABLE, (64 * 1024 * MIB).to_string())],
        )
    };
    let job = job::start(&support::manager(), plan, options).unwrap();
    let mut cancelled = None;
    // Cancelled once the flood itself is under way: the helper is this test
    // binary, whose harness prints its own lines before the helper runs (and
    // writes its pid), so cancelling on the very first output could kill it
    // before then on a slow computer.
    let ran = follow(job, Duration::from_secs(60), |job, event| {
        let flooding = matches!(event, JobEvent::Output { text, .. } if text.contains("flood"));
        if flooding && cancelled.is_none() {
            cancelled = Some(Instant::now());
            job.cancel();
        }
    })
    .await;
    let took = cancelled.expect("output came").elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert_eq!(script(&ran).state, HostState::Cancelled);
    assert_gone(pid(&pids), "the flood after cancel");
}

/// Bytes that are not UTF-8 are kept as replacement characters, a
/// character split across two writes is kept whole, and the kept run's
/// output file is UTF-8 and opens.
#[tokio::test]
async fn output_that_is_not_utf8_is_kept_readable() {
    let lab = lab();
    let python = stand_in(&lab);
    let pids = pids(&lab, "utf8");
    let plan = lab.plan(&folder(""), &step());
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
    let id = recorder.id().to_string();
    let job = job::start(
        &support::manager(),
        plan,
        options(&python, NOT_UTF8, &pids, &[]),
    )
    .unwrap();
    let ran = follow(job, Duration::from_secs(60), |_, event| {
        recorder.record(event).unwrap()
    })
    .await;
    let output = &script(&ran).output;
    assert_eq!(script(&ran).state, HostState::Done, "{output}");
    assert!(output.contains("latin-1: caf\u{fffd}\n"), "{output}");
    assert!(
        output.contains("broken: \u{fffd}\u{fffd}\u{fffd}\n"),
        "{output}"
    );
    assert!(output.contains("split: \u{20ac} euro\n"), "{output}");
    let kept = store.run(&ws, &id).unwrap();
    let file = store.root().join(&ws).join(&id).join(&kept.hosts[0].file);
    let bytes = fs::read(file).unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), *output);
}

/// A script that reads its standard input gets nothing (it is not a
/// terminal, and nobody types into a job): it does not wait for ever.
#[tokio::test]
async fn a_script_reading_its_input_gets_nothing_and_goes_on() {
    let lab = lab();
    let ran = run_role(&lab, STDIN, &[]).await;
    let result = script(&ran);
    assert_eq!(result.state, HostState::Done, "{}", result.output);
    assert!(
        result.output.contains("stdin gave 0 bytes (true)"),
        "{}",
        result.output
    );
}

/// Every way a script can end, and what the run says: exit 0 is done, any
/// other status fails naming it, a crash or a panic fails with its status
/// (or, on Unix, the signal it died of).
#[tokio::test]
async fn a_script_ending_in_every_way_says_how() {
    let lab = lab();
    for code in [0, 1, 2, 255] {
        let ran = run_role(&lab, EXIT, &[(CODE_VARIABLE, code.to_string())]).await;
        let result = script(&ran);
        assert_eq!(result.exit_status, Some(code), "{}", result.output);
        if code == 0 {
            assert_eq!(result.state, HostState::Done);
        } else {
            assert_eq!(
                result.state,
                HostState::Failed {
                    reason: format!("the script exited with status {code}")
                }
            );
        }
    }
    let ran = run_role(&lab, PANIC, &[]).await;
    assert_eq!(
        script(&ran).exit_status,
        Some(101),
        "{}",
        script(&ran).output
    );
    assert!(script(&ran).output.contains("the script panicked"));
    // A crash. On Windows a crashed program's status is negative
    // (0xC0000409 for an abort), which the stand-in's cmd.exe does not pass
    // on (it ends 0): not played there. The app starts python.exe itself,
    // whose status comes through as it is.
    if cfg!(unix) {
        let ran = run_role(&lab, ABORT, &[]).await;
        let result = script(&ran);
        assert_eq!(result.exit_status, None);
        assert_eq!(
            result.state,
            HostState::Failed {
                reason: "the script was ended without an exit status (by a signal)".into()
            }
        );
    }
}

/// A script that ignores every request to stop is ended all the same by a
/// cancel, and so is one holding a connection open (as a tunnel or the
/// credential agent's connection would be): the connection closes.
#[tokio::test]
async fn cancel_ends_a_script_that_ignores_it_and_closes_what_it_held() {
    let lab = lab();
    let python = stand_in(&lab);

    let pids_stubborn = pids(&lab, "stubborn");
    let plan = lab.plan(&folder(""), &step());
    let job = job::start(
        &support::manager(),
        plan,
        options(&python, STUBBORN, &pids_stubborn, &[]),
    )
    .unwrap();
    let ran = follow(job, Duration::from_secs(60), |job, event| {
        if let JobEvent::Output { text, .. } = event {
            if text.contains("stubborn:") {
                job.cancel();
            }
        }
    })
    .await;
    assert_eq!(script(&ran).state, HostState::Cancelled);
    assert_gone(pid(&pids_stubborn), "a script ignoring stop requests");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let pids_connect = pids(&lab, "connect");
    let plan = lab.plan(&folder(""), &step());
    let job = job::start(
        &support::manager(),
        plan,
        options(
            &python,
            CONNECT,
            &pids_connect,
            &[(PORT_VARIABLE, port.to_string())],
        ),
    )
    .unwrap();
    let accepting = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        let mut hello = [0; 5];
        socket.read_exact(&mut hello).unwrap();
        // Then nothing until the far end goes: end of file, or reset.
        let mut rest = Vec::new();
        let ended = Instant::now();
        let _ = socket.read_to_end(&mut rest);
        (hello, ended)
    });
    let ran = follow(job, Duration::from_secs(60), |job, event| {
        if let JobEvent::Output { text, .. } = event {
            if text.contains("connected") {
                job.cancel();
            }
        }
    })
    .await;
    assert_eq!(script(&ran).state, HostState::Cancelled);
    let (hello, since) = accepting.join().unwrap();
    assert_eq!(&hello, b"hello");
    assert!(
        since.elapsed() < Duration::from_secs(30),
        "the connection closed with the script"
    );
    assert_gone(pid(&pids_connect), "a script holding a connection");
}

/// A package the script needs is not installed: the run fails with the
/// script's exit status and Python's own words in its output.
#[tokio::test]
async fn a_package_the_script_needs_missing_shows_pythons_words() {
    let Some(python) = real_python() else {
        eprintln!("a missing package with a real Python SKIPPED: no Python 3.10+ found");
        return;
    };
    let lab = Lab::new(&[
        ("r1.yaml", "address: 10.0.0.1\n".into()),
        (
            "needs.py",
            "import enodix_no_such_package_119\nprint('never')\n".into(),
        ),
    ]);
    let plan = lab.plan(
        &folder(""),
        &JobStep::Script {
            path: "needs.py".into(),
        },
    );
    let options = JobOptions {
        script: Some(ScriptOptions {
            interpreter: python.executable.clone(),
            cli: None,
            environment: Vec::new(),
        }),
        ..JobOptions::default()
    };
    let job = job::start(&support::manager(), plan, options).unwrap();
    let ran = follow(job, Duration::from_secs(60), |_, _| {}).await;
    let result = script(&ran);
    assert_eq!(
        result.state,
        HostState::Failed {
            reason: "the script exited with status 1".into()
        },
        "{}",
        result.output
    );
    assert!(
        result
            .output
            .contains("ModuleNotFoundError: No module named 'enodix_no_such_package_119'"),
        "{}",
        result.output
    );
}
