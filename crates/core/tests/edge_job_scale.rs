//! Edge testing of jobs at scale (#119): hundreds of hosts against one
//! in-process device, at the default parallel limit and at the highest,
//! kept as a run and opened again; the process's memory and handles before
//! and after, and the time each piece takes (printed, for the notes).
//!
//! The 100-host run is part of the suite; the 500-host one (the ticket's
//! number) is `#[ignore]`d for its time:
//! `cargo test -p netterm-core --test edge_job_scale -- --ignored --nocapture`.

mod support;

use std::time::{Duration, Instant};

use netterm_core::job::results::{OutputRef, RunStore};
use netterm_core::job::{self, HostState, JobOptions};
use support::jobs::{closes, folder, host, snippet, Lab};
use support::memory::{self, MIB};
use support::TestSshServer;

/// `hosts` hosts on one device, run with each parallel limit in turn and
/// kept; returns nothing, asserts everything.
async fn many_hosts(hosts: usize) {
    let _alone = memory::exclusive().await;
    let device = TestSshServer::start_device("r1").await;
    let mut files = vec![(
        "_snippets.yaml".to_string(),
        "Check: \"echo {{name}} checked\\nlines 30\"\n".to_string(),
    )];
    for n in 0..hosts {
        // Ten folders of hosts, as a site would have.
        files.push((
            format!("Site{}/r{n:04}.yaml", n % 10),
            host(&device, "frr", ""),
        ));
    }
    let files: Vec<(&str, String)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.clone()))
        .collect();
    let lab = Lab::new(&files);
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let manager = support::manager();

    let resident_before = memory::resident();
    let mut handles = Vec::new();
    // The last run again, to see that nothing grows from run to run (the
    // first warms the runtime's threads up).
    for parallel in [job::DEFAULT_PARALLEL, job::MAX_PARALLEL, job::MAX_PARALLEL] {
        let planning = Instant::now();
        let plan = lab.plan(&folder(""), &snippet("Check"));
        let planned = planning.elapsed();
        assert_eq!(plan.hosts.len(), hosts);
        assert!(plan.skipped.is_empty());

        let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
        let id = recorder.id().to_string();
        let peak = memory::Peak::start();
        let running = Instant::now();
        let options = JobOptions {
            parallel,
            ..JobOptions::default()
        };
        let mut job = job::start(&manager, plan, options).unwrap();
        let mut recording = Duration::ZERO;
        let mut report = None;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(300), job.next())
            .await
            .expect("the run ends")
        {
            let at = Instant::now();
            recorder.record(&event).unwrap();
            recording += at.elapsed();
            if let job::JobEvent::Finished(done) = event {
                report = Some(done);
            }
        }
        let ran = running.elapsed();
        let highest = peak.stop();
        let report = report.expect("a report");

        // Every host done, once, with its own output.
        assert_eq!(report.hosts.len(), hosts);
        for result in &report.hosts {
            assert_eq!(
                result.state,
                HostState::Done,
                "{}: {}",
                result.id,
                result.output
            );
            assert!(
                result
                    .output
                    .contains(&format!("\n{} checked\n", result.name)),
                "{}: {}",
                result.id,
                result.output
            );
        }
        assert!(device.most_connections_open() <= parallel);

        // The kept run opens again, whole.
        let opening = Instant::now();
        let kept = store.run(&ws, &id).unwrap();
        assert!(kept.ended());
        assert_eq!(kept.hosts.len(), hosts);
        assert!(kept.hosts.iter().all(|host| host.state == HostState::Done));
        for result in &report.hosts {
            let output = store
                .output(
                    &ws,
                    &OutputRef {
                        run: id.clone(),
                        host: result.id.clone(),
                    },
                )
                .unwrap();
            assert_eq!(output, result.output, "{}", result.id);
        }
        let opened = opening.elapsed();
        let searching = Instant::now();
        let hits = store.search(&ws, &id, "line 30").unwrap();
        assert_eq!(hits.len(), hosts);
        let searched = searching.elapsed();
        let grew = highest.saturating_sub(resident_before.unwrap_or(0));
        println!(
            "{hosts} hosts, {parallel} at once: planned in {planned:?}, ran in {ran:?} (recording {recording:?}), opened again in {opened:?}, searched in {searched:?}; memory peak +{} MiB",
            grew / MIB
        );
        // Bounded: far less than a megabyte per host held.
        if resident_before.is_some() {
            assert!(
                grew < 64 * MIB + hosts as u64 * MIB / 4,
                "memory grew {} MiB",
                grew / MIB
            );
        }
        closes(&device, Duration::from_secs(30)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        handles.push(memory::open_handles());
    }
    println!("handles after each run: {handles:?}");
    // Nothing left open from run to run: connections, files.
    if let [_, Some(second), Some(third)] = handles[..] {
        assert!(
            third <= second + 16,
            "handles grew from {second} to {third} over one run"
        );
    }
    let listed = store.list(&ws).unwrap();
    assert_eq!(listed.runs.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hundred_hosts_run_kept_and_opened_again() {
    many_hosts(100).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: 500 hosts twice (about a minute in a debug build)"]
async fn five_hundred_hosts_run_kept_and_opened_again() {
    many_hosts(500).await;
}
