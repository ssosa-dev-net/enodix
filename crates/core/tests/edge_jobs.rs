//! Edge testing of snippet jobs (#119): hosts failing in every way at once,
//! cancelling at every stage, slow and changing prompts, two runs at once
//! on the same hosts, secrets, and what the review of #112 found untested
//! (#127: the connect timeout, legacy crypto, an open terminal left alone).
//! Hosts are in-process device-mode SSH servers (`support`); timeouts are
//! short, so the bounds below are what the run may take at most on a
//! loaded machine, never a sleep.

mod support;

use std::fs;
use std::time::Duration;

use netterm_core::credentials::CredentialProfile;
use netterm_core::job::results::{OutputRef, RunStore};
use netterm_core::job::{
    self, HostState, JobEvent, JobOptions, JobStep, JobTarget, SkipReason, HIDDEN_SECRET,
};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::{SessionManager, SessionState, SessionTarget, TerminalSize};
use netterm_core::workspace::HostId;
use support::jobs::{closes, folder, follow, host, host_at, profile, run, snippet, Lab};
use support::{
    next_state, output_ending_with, SilentServer, TestSshServer, ENABLE_SECRET, OTP_USERNAME,
    PASSWORD, STALLING_USERNAME, USERNAME, WAIT,
};

/// Short timeouts, so that every way of failing ends quickly.
fn quick() -> JobOptions {
    JobOptions {
        parallel: job::MAX_PARALLEL,
        command_timeout: Duration::from_secs(1),
        command_limit: Duration::from_secs(2),
        ..JobOptions::default()
    }
}

/// A platform with no disable-paging command, and one whose privilege
/// command never gives the prompt back (to cancel during that step).
const PLATFORMS: &str =
    "pager:\n  prompt: '[\\w.\\-]+#'\nstuck:\n  prompt: '[\\w.\\-]+#'\n  privilege_command: hang\n";

/// The snippet every host runs: its own command.
const SNIPPETS: &str = "Run: \"{{command}}\"\n";

fn command(text: &str) -> String {
    format!("custom:\n  command: \"{text}\"\n")
}

/// One run in which every host fails in a way of its own, beside one that
/// works: each ends with its own reason, none holds the run open past its
/// timeout, and the counts add up.
#[tokio::test]
async fn hosts_failing_in_every_way_at_once_each_end_with_their_own_reason() {
    let device = TestSshServer::start_device("r1").await;
    let unknown = TestSshServer::start_device("unknown").await;
    let changed = TestSshServer::start_device("changed").await;
    let unprivileged = TestSshServer::start_unprivileged_device("u1").await;
    let silent = SilentServer::start().await;
    let no_shell = TestSshServer::start_device("noshell").await;
    no_shell
        .quirks()
        .misbehave_at(support::Stage::ShellRequest, support::Misbehaviour::Stall);
    let closed = support::closed_port().await;
    let down_jump = support::closed_port().await;

    let at_device = |extra: &str| host(&device, "frr", extra);
    let mut lab = Lab::new(&[
        ("_platforms.yaml", PLATFORMS.into()),
        ("_snippets.yaml", SNIPPETS.into()),
        ("ok.yaml", at_device(&command("show hostname"))),
        (
            "no-profile.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\nplatform: frr\n",
                device.port()
            ),
        ),
        (
            "wrong-password.yaml",
            at_device("credential_profile: wrong\n"),
        ),
        ("unknown-key.yaml", host(&unknown, "frr", "")),
        ("changed-key.yaml", host(&changed, "frr", "")),
        ("question.yaml", at_device("credential_profile: otp\n")),
        ("refused.yaml", host_at(closed, "frr", "")),
        (
            "silent.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\nplatform: frr\ncredential_profile: lab\nconnect_timeout: 1\n",
                silent.port()
            ),
        ),
        (
            "stalled-login.yaml",
            at_device("credential_profile: stalled\nconnect_timeout: 1\n"),
        ),
        ("no-shell.yaml", host(&no_shell, "frr", "connect_timeout: 1\n")),
        (
            "unresolvable.yaml",
            "address: no-such-device.invalid\nplatform: frr\ncredential_profile: lab\n".into(),
        ),
        ("jump-down.yaml", host_at(down_jump, "frr", "")),
        ("behind.yaml", at_device("jump_host: jump-down.yaml\n")),
        ("dropped.yaml", at_device(&command("drop"))),
        ("never.yaml", at_device(&command("hang"))),
        ("pager.yaml", host(&device, "pager", &command("lines 60"))),
        (
            "wrong-enable.yaml",
            host(&unprivileged, "eos", "credential_profile: wrong-enable\n"),
        ),
        ("endless.yaml", at_device(&command("flood"))),
        ("unknown-command.yaml", at_device(&command("frobnicate"))),
    ]);
    lab.put(CredentialProfile {
        auth: netterm_core::credentials::ProfileAuth::Password {
            password: "not the password".into(),
        },
        ..profile("wrong", USERNAME)
    });
    lab.put(profile("otp", OTP_USERNAME));
    lab.put(profile("stalled", STALLING_USERNAME));
    lab.put(CredentialProfile {
        enable_secret: Some("not the secret".into()),
        ..profile("wrong-enable", USERNAME)
    });
    lab.reload();

    // Every server but `unknown` is trusted; `changed` with another's key.
    let known_hosts = KnownHosts::in_memory();
    for server in [&device, &changed, &unprivileged, &no_shell] {
        known_hosts
            .remember("127.0.0.1", server.port(), server.host_key())
            .unwrap();
    }
    known_hosts
        .remember("127.0.0.1", changed.port(), device.host_key())
        .unwrap();

    // Hosts with no command of their own show their hostname.
    let step = JobStep::Snippet {
        name: "Run".into(),
        answers: [("command".to_string(), "show hostname".to_string())].into(),
    };
    let plan = lab.plan(&folder(""), &step);
    let skipped: Vec<(&str, &SkipReason)> = plan
        .skipped
        .iter()
        .map(|host| (host.id.as_str(), &host.reason))
        .collect();
    assert_eq!(skipped, [("no-profile.yaml", &SkipReason::NoProfile)]);
    // The jump host itself is a host of the folder too.
    let hosts = plan.hosts.len();
    assert_eq!(hosts + plan.skipped.len(), 19, "every host is counted once");

    let manager = SessionManager::new(known_hosts);
    let ran = run(&manager, plan, quick(), Duration::from_secs(60)).await;

    let reason = |id: &str| ran.failure(id);
    assert_eq!(*ran.state("ok.yaml"), HostState::Done);
    assert!(ran.result("ok.yaml").output.contains("\nr1\n"));
    // A line the device does not know still got its prompt back.
    assert_eq!(*ran.state("unknown-command.yaml"), HostState::Done);
    let expect = [
        ("wrong-password.yaml", "authentication failed"),
        ("unknown-key.yaml", "is not trusted yet"),
        ("changed-key.yaml", "has changed since it was trusted"),
        ("question.yaml", "a job never answers login questions"),
        ("refused.yaml", "refused"),
        ("silent.yaml", "1 s"),
        ("stalled-login.yaml", "1 s"),
        ("no-shell.yaml", "1 s"),
        ("unresolvable.yaml", "could not be resolved"),
        ("jump-down.yaml", "refused"),
        ("behind.yaml", "jump-down.yaml"),
        ("dropped.yaml", "the session ended after \"drop\""),
        (
            "never.yaml",
            "no prompt after \"hang\": the device showed nothing more for 1 s",
        ),
        (
            "pager.yaml",
            "no prompt after \"lines 60\": the device showed nothing more for 1 s",
        ),
        (
            "wrong-enable.yaml",
            "refused the enable secret of the credential profile wrong-enable",
        ),
        (
            "endless.yaml",
            "no prompt after \"flood\": it was still showing output after 2 s",
        ),
    ];
    for (id, words) in expect {
        let why = reason(id);
        assert!(why.contains(words), "{id}: {why}");
    }
    // Each its own reason: no two failures read the same, but for the
    // three a connect timeout ends (no banner, a login never answered, a
    // shell never given), which all say the device did not answer in time.
    let timeouts = ["silent.yaml", "stalled-login.yaml", "no-shell.yaml"];
    for id in timeouts {
        assert_eq!(reason(id), "the device did not answer within 1 s");
    }
    let mut reasons: Vec<String> = expect
        .iter()
        .filter(|(id, _)| !timeouts.contains(id))
        .map(|(id, _)| reason(id))
        .collect();
    reasons.sort();
    let all = reasons.len();
    reasons.dedup();
    assert_eq!(reasons.len(), all, "{reasons:#?}");

    // The counts add up.
    let done = ran
        .report
        .hosts
        .iter()
        .filter(|host| host.state == HostState::Done)
        .count();
    let failed = ran
        .report
        .hosts
        .iter()
        .filter(|host| matches!(host.state, HostState::Failed { .. }))
        .count();
    assert_eq!((done, failed), (2, hosts - 2), "{:#?}", ran.report.hosts);

    // Nobody held the run open past its own timeout: 1 s of quiet, 2 s of
    // output, 1 s to connect, plus closing (2 s at most) and a busy machine.
    for host in &ran.report.hosts {
        let at = ran.ended_at(host.id.as_str());
        assert!(
            at < Duration::from_secs(15),
            "{} ended after {at:?}",
            host.id
        );
    }
    assert!(ran.took < Duration::from_secs(20), "{:?}", ran.took);

    // No password and no secret in anything the run said.
    let said =
        serde_json::to_string(&ran.report).unwrap() + &serde_json::to_string(&ran.events).unwrap();
    for secret in [PASSWORD, "not the password", "not the secret"] {
        assert!(!said.contains(secret), "{secret:?} was shown");
    }
    // (`no_shell` is stalled inside the shell request: its side stays.)
    for server in [&device, &unprivileged] {
        closes(server, WAIT).await;
    }
}

/// Cancelling with a host at each stage: done, connecting, in the
/// privilege step, in the middle of output, waiting for a prompt, waiting
/// for room. Cancelled
/// twice. Every session closes, the kept run says which hosts were
/// cancelled, and the run can be planned and started again.
#[tokio::test]
async fn cancelling_with_hosts_at_every_stage_ends_them_all_and_can_be_run_again() {
    let device = TestSshServer::start_device("r1").await;
    let silent = SilentServer::start().await;
    let lab = Lab::new(&[
        ("_platforms.yaml", PLATFORMS.into()),
        ("_snippets.yaml", SNIPPETS.into()),
        ("a-done.yaml", host(&device, "frr", &command("echo done"))),
        (
            "b-connecting.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\nplatform: frr\ncredential_profile: lab\nconnect_timeout: 60\ncustom:\n  command: x\n",
                silent.port()
            ),
        ),
        (
            "c-privilege.yaml",
            host(&device, "stuck", &command("echo never")),
        ),
        ("d-output.yaml", host(&device, "frr", &command("flood"))),
        // Takes the room the done host leaves.
        ("e-hanging.yaml", host(&device, "frr", &command("hang"))),
        ("f-waiting.yaml", host(&device, "frr", &command("echo never"))),
    ]);
    let plan = lab.plan(&folder(""), &snippet("Run"));
    assert_eq!(plan.hosts.len(), 6);
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
    let run_id = recorder.id().to_string();

    let manager = support::manager();
    let options = JobOptions {
        parallel: 4,
        ..JobOptions::default()
    };
    let job = job::start(&manager, plan, options).unwrap();
    let mut done = false;
    let mut privileged = false;
    let mut flooding = false;
    let mut cancelled_at = None;
    let ran = follow(job, Duration::from_secs(60), |job, event| {
        recorder.record(event).unwrap();
        match event {
            JobEvent::State { host, state } if host.as_str() == "a-done.yaml" => {
                done |= *state == HostState::Done
            }
            JobEvent::Output { host, text } if host.as_str() == "c-privilege.yaml" => {
                privileged |= text.contains("r1# hang")
            }
            JobEvent::Output { host, text } if host.as_str() == "d-output.yaml" => {
                flooding |= text.contains("flood flood")
            }
            _ => {}
        }
        if done && privileged && flooding && cancelled_at.is_none() {
            cancelled_at = Some(std::time::Instant::now());
            job.cancel();
            job.cancel();
            job.canceller().cancel();
        }
    })
    .await;
    let cancelled_at = cancelled_at.expect("every stage was reached");
    let ending = cancelled_at.elapsed();
    assert!(ending < Duration::from_secs(10), "cancel took {ending:?}");

    assert_eq!(*ran.state("a-done.yaml"), HostState::Done);
    for id in [
        "b-connecting.yaml",
        "c-privilege.yaml",
        "d-output.yaml",
        "e-hanging.yaml",
        "f-waiting.yaml",
    ] {
        assert_eq!(*ran.state(id), HostState::Cancelled, "{id}");
    }
    let states = ran.states();
    assert_eq!(
        states[&HostId::new("b-connecting.yaml")],
        [
            HostState::Waiting,
            HostState::Connecting,
            HostState::Cancelled
        ]
    );
    assert_eq!(
        states[&HostId::new("f-waiting.yaml")],
        [HostState::Waiting, HostState::Cancelled],
        "a host waiting for room never connects"
    );
    assert!(ran
        .result("d-output.yaml")
        .output
        .contains("r1# flood\nflood flood"));
    assert!(ran.result("c-privilege.yaml").output.contains("r1# hang"));
    closes(&device, WAIT).await;

    // The kept run: ended, each host as the report says, no reason given
    // (the user cancelled), and it can be planned again and started.
    let kept = store.run(&ws, &run_id).unwrap();
    assert!(kept.ended());
    assert_eq!(kept.cancel_reason, None);
    let kept_states: Vec<(&str, &HostState)> = kept
        .hosts
        .iter()
        .map(|host| (host.id.as_str(), &host.state))
        .collect();
    assert_eq!(
        kept_states,
        [
            ("a-done.yaml", &HostState::Done),
            ("b-connecting.yaml", &HostState::Cancelled),
            ("c-privilege.yaml", &HostState::Cancelled),
            ("d-output.yaml", &HostState::Cancelled),
            ("e-hanging.yaml", &HostState::Cancelled),
            ("f-waiting.yaml", &HostState::Cancelled),
        ]
    );
    let output = store
        .output(
            &ws,
            &OutputRef {
                run: run_id.clone(),
                host: HostId::new("d-output.yaml"),
            },
        )
        .unwrap();
    assert_eq!(output, ran.result("d-output.yaml").output);
    let again = job::plan_again(
        &lab.workspace,
        &netterm_core::credentials::Profiles::new(&lab.store, &lab.workspace),
        &kept,
    )
    .unwrap();
    assert_eq!(again.hosts.len(), 6);
    let job = job::start(&manager, again, JobOptions::default()).unwrap();
    job.cancel();
    let ran = follow(job, Duration::from_secs(30), |_, _| {}).await;
    assert!(ran
        .report
        .hosts
        .iter()
        .all(|host| host.state == HostState::Cancelled));
}

/// A cancel that comes as the last host finishes, after its final state but
/// before the report, changes nothing: every host is done and the kept run
/// says nothing was cancelled. A cancel after the end is ignored.
#[tokio::test]
async fn a_cancel_as_the_last_host_finishes_changes_nothing() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[
        ("_snippets.yaml", SNIPPETS.into()),
        ("a.yaml", host(&device, "frr", &command("echo a"))),
        ("b.yaml", host(&device, "frr", &command("pause 200"))),
    ]);
    let plan = lab.plan(&folder(""), &snippet("Run"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
    let id = recorder.id().to_string();
    let manager = support::manager();
    let job = job::start(&manager, plan, JobOptions::default()).unwrap();
    let canceller = job.canceller();
    let mut finished = 0;
    let ran = follow(job, WAIT * 3, |job, event| {
        recorder.record(event).unwrap();
        if let JobEvent::State { state, .. } = event {
            if state.is_final() {
                finished += 1;
                if finished == 2 {
                    job.cancel();
                }
            }
        }
    })
    .await;
    canceller.cancel();
    assert!(ran
        .report
        .hosts
        .iter()
        .all(|host| host.state == HostState::Done));
    let kept = store.run(&ws, &id).unwrap();
    assert!(kept.ended());
    assert!(kept.hosts.iter().all(|host| host.state == HostState::Done));
    assert_eq!(kept.cancel_reason, None);
}

/// Two runs at once on the same hosts, kept in one store: each its own run
/// id and output files, neither seeing the other's output.
#[tokio::test]
async fn two_runs_at_once_on_the_same_hosts_keep_their_own_output() {
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let lab = Lab::new(&[
        (
            "_snippets.yaml",
            "First: echo first {{name}}\nSecond: \"drip 3 100\\necho second {{name}}\"\n".into(),
        ),
        ("r1.yaml", host(&r1, "frr", "")),
        ("r2.yaml", host(&r2, "frr", "")),
    ]);
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let manager = support::manager();
    let mut runs = Vec::new();
    let mut tasks = Vec::new();
    for name in ["First", "Second"] {
        let plan = lab.plan(&folder(""), &snippet(name));
        let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
        runs.push((name, recorder.id().to_string()));
        let mut job = job::start(&manager, plan, JobOptions::default()).unwrap();
        tasks.push(tokio::spawn(async move {
            while let Some(event) = job.next().await {
                recorder.record(&event).unwrap();
            }
        }));
    }
    for task in tasks {
        tokio::time::timeout(WAIT * 3, task)
            .await
            .expect("both runs end")
            .unwrap();
    }
    assert_ne!(runs[0].1, runs[1].1);
    let listed = store.list(&ws).unwrap();
    assert_eq!(listed.runs.len(), 2);
    assert!(listed.damaged.is_empty());
    for (name, run) in &runs {
        let kept = store.run(&ws, run).unwrap();
        assert!(kept.ended());
        for host in ["r1", "r2"] {
            let output = store
                .output(
                    &ws,
                    &OutputRef {
                        run: run.clone(),
                        host: HostId::new(format!("{host}.yaml")),
                    },
                )
                .unwrap();
            let word = name.to_lowercase();
            let other = if word == "first" { "second" } else { "first" };
            assert!(
                output.contains(&format!("\n{word} {host}\n")),
                "{run} {host}: {output}"
            );
            assert!(!output.contains(other), "{run} {host}: {output}");
        }
    }
}

/// Two hosts whose files differ only in their extension (`r1.yaml`,
/// `r1.yml`) keep an output file each in a kept run (#127: they shared
/// one, the second's output written over the first's).
#[tokio::test]
async fn hosts_with_the_same_file_stem_keep_an_output_file_each() {
    let a = TestSshServer::start_device("alpha").await;
    let b = TestSshServer::start_device("bravo").await;
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        ("Site/r1.yaml", host(&a, "frr", "")),
        ("Site/r1.yml", host(&b, "frr", "")),
    ]);
    let plan = lab.plan(&folder("Site"), &snippet("Show"));
    assert_eq!(plan.hosts.len(), 2);
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder("Site"), &plan).unwrap();
    let id = recorder.id().to_string();
    let manager = support::manager();
    let mut job = job::start(&manager, plan, JobOptions::default()).unwrap();
    while let Some(event) = job.next().await {
        recorder.record(&event).unwrap();
    }
    let kept = store.run(&ws, &id).unwrap();
    assert_ne!(kept.hosts[0].file, kept.hosts[1].file, "{:?}", kept.hosts);
    for (host, name) in [("Site/r1.yaml", "alpha"), ("Site/r1.yml", "bravo")] {
        let output = store
            .output(
                &ws,
                &OutputRef {
                    run: id.clone(),
                    host: HostId::new(host),
                },
            )
            .unwrap();
        assert!(output.contains(&format!("\n{name}\n")), "{host}: {output}");
    }
}

/// A prompt that changes (into config mode and out, or to another name) is
/// followed: the definition's pattern decides, not the first prompt seen.
#[tokio::test]
async fn a_prompt_that_changes_is_followed() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[
        (
            "_snippets.yaml",
            "Modes: \"prompt r1(config)\\necho in config\\nprompt core-2\\necho renamed\\nprompt\\necho back\"\n".into(),
        ),
        ("r1.yaml", host(&device, "frr", "")),
    ]);
    let plan = lab.plan(&folder(""), &snippet("Modes"));
    let ran = run(&support::manager(), plan, quick(), WAIT * 3).await;
    let r1 = ran.result("r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert_eq!(
        r1.output,
        "r1# terminal length 0\nr1# prompt r1(config)\nr1(config)# echo in config\nin config\nr1(config)# prompt core-2\ncore-2# echo renamed\nrenamed\ncore-2# prompt\nr1# echo back\nback\nr1#\n"
    );
}

/// A console that answers a byte at a time, faster or slower than the
/// prompt must stay quiet to count (300 ms), and one that answers only
/// after seconds: each line is answered whole before the next is typed.
#[tokio::test]
async fn slow_consoles_are_waited_for() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[
        (
            "_snippets.yaml",
            "Crawl: \"crawl 40 a slow answer\\necho after crawl\"\nCreep: \"crawl 450 tick\\necho after creep\"\nLate: \"stall 2500 late answer\\necho after pause\"\n".into(),
        ),
        ("r1.yaml", host(&device, "frr", "")),
    ]);
    let options = JobOptions {
        command_timeout: Duration::from_secs(5),
        ..JobOptions::default()
    };
    for (name, answer, after) in [
        ("Crawl", "a slow answer", "after crawl"),
        ("Creep", "tick", "after creep"),
        ("Late", "lateanswer", "after pause"),
    ] {
        let plan = lab.plan(&folder(""), &snippet(name));
        let ran = run(&support::manager(), plan, options.clone(), WAIT * 3).await;
        let r1 = ran.result("r1.yaml");
        assert_eq!(r1.state, HostState::Done, "{name}: {}", r1.output);
        let answered = r1.output.find(&format!("\n{answer}")).unwrap_or(0);
        let next = r1
            .output
            .find(&format!("echo {after}"))
            .unwrap_or_else(|| panic!("{name}: {}", r1.output));
        assert!(answered < next, "{name}: {}", r1.output);
        assert!(
            r1.output.contains(&format!("\n{after}\n")),
            "{name}: {}",
            r1.output
        );
    }
    // Answering after the command timeout fails, saying how long it waited.
    let plan = lab.plan(&folder(""), &snippet("Late"));
    let ran = run(&support::manager(), plan, quick(), WAIT * 3).await;
    assert_eq!(
        ran.failure("r1.yaml"),
        "no prompt after \"stall 2500 late answer\": the device showed nothing more for 1 s"
    );
}

/// A device that pauses half way through a line that ends like a prompt
/// (`total#`), longer than the prompt must stay quiet to count, then goes
/// on. The job takes the pause for the prompt and types the next line
/// while the device is still answering: a known limit of prompt detection
/// by pattern and quiet (see the report of #119).
#[tokio::test]
#[ignore = "#119 finding: a pause after text that looks like the prompt ends the command early (ticket to be written)"]
async fn a_pause_after_text_like_the_prompt_does_not_end_the_command() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[
        (
            "_snippets.yaml",
            "Stall: \"stall 1500 total# 42 routes\\necho after\"\n".into(),
        ),
        ("r1.yaml", host(&device, "frr", "")),
    ]);
    let plan = lab.plan(&folder(""), &snippet("Stall"));
    let options = JobOptions {
        command_timeout: Duration::from_secs(5),
        ..JobOptions::default()
    };
    let ran = run(&support::manager(), plan, options, WAIT * 3).await;
    let r1 = ran.result("r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert!(
        r1.output.contains("total#42 routes\nr1# echo after\n"),
        "{}",
        r1.output
    );
}

/// The connect timeout a host has is the one its job uses (#127): a device
/// that never speaks fails after it, not after the default 15 s.
#[tokio::test]
async fn a_job_uses_the_hosts_connect_timeout() {
    let silent = SilentServer::start().await;
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        (
            "silent.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\nplatform: frr\ncredential_profile: lab\nconnect_timeout: 2\n",
                silent.port()
            ),
        ),
    ]);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let ran = run(&support::manager(), plan, JobOptions::default(), WAIT * 3).await;
    let reason = ran.failure("silent.yaml");
    assert!(reason.contains("2 s"), "{reason}");
    let at = ran.ended_at("silent.yaml");
    assert!(
        at >= Duration::from_secs(2) && at < Duration::from_secs(10),
        "{at:?}"
    );
}

/// A host set to allow legacy crypto reaches an old device in a job (#127);
/// without it, the job fails it with the negotiation's advice.
#[tokio::test]
async fn a_job_uses_the_hosts_legacy_crypto_setting() {
    let old =
        TestSshServer::start_device_with_algorithms("old1", support::algorithms::old_device())
            .await;
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        ("legacy.yaml", host(&old, "frr", "legacy_crypto: true\n")),
        ("modern.yaml", host(&old, "frr", "")),
    ]);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let ran = run(&support::manager(), plan, JobOptions::default(), WAIT * 3).await;
    let legacy = ran.result("legacy.yaml");
    assert_eq!(legacy.state, HostState::Done, "{}", legacy.output);
    assert!(legacy.output.contains("\nold1\n"), "{}", legacy.output);
    let reason = ran.failure("modern.yaml");
    assert!(reason.to_lowercase().contains("legacy"), "{reason}");
}

/// A job on a host that has a terminal open leaves that terminal alone
/// (#127): the job connects on its own, the terminal sees none of its
/// output and still answers afterwards.
#[tokio::test]
async fn a_job_leaves_an_open_terminal_to_the_same_host_untouched() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[
        ("_snippets.yaml", "Mark: echo from the job\n".into()),
        ("r1.yaml", host(&device, "frr", "")),
    ]);
    let manager = support::manager();
    let size = TerminalSize { cols: 80, rows: 24 };
    let (terminal, mut events) = manager.open(SessionTarget::Ssh(device.target()), size);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, &device.prompt()).await;

    let plan = lab.plan(&folder(""), &snippet("Mark"));
    let ran = run(&manager, plan, JobOptions::default(), WAIT * 3).await;
    assert_eq!(*ran.state("r1.yaml"), HostState::Done);
    assert_eq!(
        device.connections(),
        2,
        "the job had a connection of its own"
    );

    // Nothing of the job reached the terminal; it still answers.
    manager
        .write(terminal, b"echo still here\r".to_vec())
        .unwrap();
    let seen = output_ending_with(&mut events, &device.prompt()).await;
    let seen = String::from_utf8_lossy(&seen);
    assert!(seen.contains("still here"), "{seen}");
    assert!(!seen.contains("from the job"), "{seen}");
    assert!(!seen.contains("terminal length"), "{seen}");
}

/// A device that echoes what is typed at the enable question would show
/// the secret: the job hides it in the output, the events and the kept
/// run. The plan, its pre-flight as the interface gets it, and every
/// reason never show it nor the password.
#[tokio::test]
async fn the_enable_secret_and_the_password_are_never_shown() {
    let device = TestSshServer::start_unprivileged_device("r1").await;
    device.echo_enable_secret();
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        (
            "r1.yaml",
            host(&device, "eos", "credential_profile: enable\n"),
        ),
    ]);
    lab.put(CredentialProfile {
        enable_secret: Some(ENABLE_SECRET.into()),
        ..profile("enable", USERNAME)
    });
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let shown = serde_json::to_string(&plan).unwrap() + &format!("{plan:?}");
    for secret in [ENABLE_SECRET, PASSWORD] {
        assert!(!shown.contains(secret), "the plan shows {secret:?}");
    }
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
    let id = recorder.id().to_string();
    let job = job::start(&support::manager(), plan, JobOptions::default()).unwrap();
    let ran = follow(job, WAIT * 3, |_, event| recorder.record(event).unwrap()).await;
    let r1 = ran.result("r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert!(
        r1.output
            .starts_with(&format!("r1> enable\nPassword: {HIDDEN_SECRET}\nr1# ")),
        "{}",
        r1.output
    );
    let said = serde_json::to_string(&ran.events).unwrap();
    let kept = fs::read_to_string(
        store
            .root()
            .join(&ws)
            .join(&id)
            .join(&store.run(&ws, &id).unwrap().hosts[0].file),
    )
    .unwrap();
    for text in [&r1.output, &said, &kept] {
        assert!(!text.contains(ENABLE_SECRET), "{text}");
        assert!(!text.contains(PASSWORD), "{text}");
    }
}

/// Planning connects to nothing and returns at once, so there is nothing
/// to cancel while it runs; a plan for a target that is gone is refused
/// rather than run (the hosts of a removed folder).
#[test]
fn a_plan_for_a_target_that_is_gone_is_refused() {
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        ("Site/r1.yaml", host_at(22, "frr", "")),
    ]);
    let gone = job::plan(
        &lab.workspace,
        &netterm_core::credentials::Profiles::new(&lab.store, &lab.workspace),
        &folder("Elsewhere"),
        &snippet("Show"),
    );
    assert!(gone.is_err(), "{gone:?}");
    let ids = JobTarget::Hosts {
        ids: vec![HostId::new("Site/gone.yaml")],
    };
    let plan = lab.plan(&ids, &snippet("Show"));
    assert!(plan.hosts.is_empty());
    assert_eq!(plan.skipped[0].reason, SkipReason::Gone);
    assert!(matches!(
        job::check(&plan, &JobOptions::default()),
        Err(job::StartError::NothingToRun)
    ));
}
