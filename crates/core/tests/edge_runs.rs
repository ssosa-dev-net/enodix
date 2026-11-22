//! Edge testing of kept runs (#119): a description cut at every point or
//! damaged, output that is not UTF-8, a run deleted while it is open, and
//! thousands of runs listed. Runs are made with the store's own `begin`
//! and `record`, from plans that connect to nothing.

mod support;

use std::fs;
use std::time::{Duration, Instant, SystemTime};

use enodix_core::job::results::{
    OutputRef, ResultsError, RunList, RunStore, StoredRun, RUN_FILE, SAVE_EVERY,
};
use enodix_core::job::{HostResult, HostState, JobEvent, JobPlan, JobReport};
use enodix_core::workspace::HostId;
use support::jobs::{folder, host_at, snippet, Lab};

/// A workspace of `hosts` hosts (never connected to) with a snippet.
fn lab(hosts: usize) -> Lab {
    let mut files = vec![(
        "_snippets.yaml".to_string(),
        "Show: show hostname\n".to_string(),
    )];
    for n in 0..hosts {
        files.push((format!("r{n}.yaml"), host_at(22, "frr", "")));
    }
    let files: Vec<(&str, String)> = files.iter().map(|(p, t)| (p.as_str(), t.clone())).collect();
    Lab::new(&files)
}

/// What a run of `plan` would report: every host done with `output`.
fn report(plan: &JobPlan, output: &str) -> JobReport {
    JobReport {
        hosts: plan
            .hosts
            .iter()
            .map(|host| HostResult {
                id: host.id.clone(),
                name: host.name.clone(),
                state: HostState::Done,
                output: output.to_string(),
                backup: None,
                exit_status: None,
                facts: None,
                bytes: None,
            })
            .collect(),
        file: None,
    }
}

/// Keeps one ended run of `plan` in `store`; its id.
fn keep(store: &RunStore, ws: &str, plan: &JobPlan, output: &str) -> String {
    let mut recorder = store.begin(ws, &folder(""), plan).unwrap();
    for host in &plan.hosts {
        for state in [HostState::Connecting, HostState::Running] {
            recorder
                .record(&JobEvent::State {
                    host: host.id.clone(),
                    state,
                })
                .unwrap();
        }
        recorder
            .record(&JobEvent::Output {
                host: host.id.clone(),
                text: output.to_string(),
            })
            .unwrap();
    }
    recorder
        .record(&JobEvent::Finished(report(plan, output)))
        .unwrap();
    recorder.id().to_string()
}

/// A description cut short at any byte, as a disk filling up or a copy
/// stopped half way would leave it (the app itself writes it whole, by a
/// rename): the run is listed whole or as damaged with a reason, never
/// lost silently, and the other runs are listed as ever. A leftover
/// temporary file beside a good description changes nothing; a folder
/// with no description, or a description that is not text, is damaged.
#[test]
fn a_description_cut_anywhere_is_listed_damaged_or_whole() {
    let lab = lab(3);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let good = keep(&store, &ws, &plan, "r1#\n");
    let folder_of = |run: &str| store.root().join(&ws).join(run);
    let text = fs::read(folder_of(&good).join(RUN_FILE)).unwrap();
    fs::write(folder_of(&good).join(format!(".{RUN_FILE}.tmp")), "half").unwrap();

    let mut cuts = Vec::new();
    for at in (0..text.len()).step_by(7).chain([text.len() - 1]) {
        let id = format!("2000-01-01_00-00-00_{}", at + 2);
        let copy = folder_of(&id);
        fs::create_dir_all(copy.join("output")).unwrap();
        fs::write(copy.join(RUN_FILE), &text[..at]).unwrap();
        cuts.push(id);
    }
    let no_description = folder_of("2000-01-02_00-00-00");
    fs::create_dir_all(&no_description).unwrap();
    let not_text = folder_of("2000-01-03_00-00-00");
    fs::create_dir_all(&not_text).unwrap();
    fs::write(not_text.join(RUN_FILE), [0xff, 0xfe, 0x00, 0x41]).unwrap();

    let listed = store.list(&ws).unwrap();
    assert!(listed.runs.iter().any(|run| run.id == good));
    let good_run = store.run(&ws, &good).unwrap();
    assert!(good_run.ended());
    for id in &cuts {
        let whole = listed.runs.iter().any(|run| &run.id == id);
        let damaged = listed.damaged.iter().find(|run| &run.id == id);
        assert!(whole != damaged.is_some(), "{id} is listed once");
        if let Some(damaged) = damaged {
            assert!(!damaged.reason.trim().is_empty(), "{id}");
        }
        // Whatever was read of it opens as it was listed.
        match store.run(&ws, id) {
            Ok(_) => assert!(whole, "{id}"),
            Err(ResultsError::Damaged { .. }) | Err(ResultsError::NoSuchRun(_)) => {
                assert!(!whole, "{id}")
            }
            Err(other) => panic!("{id}: {other}"),
        }
    }
    for (id, words) in [
        ("2000-01-02_00-00-00", "has no run.yaml"),
        ("2000-01-03_00-00-00", ""),
    ] {
        let damaged = listed
            .damaged
            .iter()
            .find(|run| run.id == id)
            .unwrap_or_else(|| panic!("{id} is not listed damaged: {listed:?}"));
        assert!(damaged.reason.contains(words), "{id}: {}", damaged.reason);
    }
    // A description naming an output file outside its run is damaged, not
    // read.
    let outside = folder_of("2000-01-04_00-00-00");
    fs::create_dir_all(&outside).unwrap();
    let escaping = String::from_utf8(text.clone())
        .unwrap()
        .replace("file: output/r0.txt", "file: ../../../secrets.txt");
    fs::write(outside.join(RUN_FILE), escaping).unwrap();
    let error = store
        .output(
            &ws,
            &OutputRef {
                run: "2000-01-04_00-00-00".into(),
                host: HostId::new("r0.yaml"),
            },
        )
        .unwrap_err();
    assert!(matches!(error, ResultsError::Damaged { .. }), "{error}");
}

/// Output files that are not UTF-8 (written by something else, or a
/// device's bytes kept by an older version) still open, search and
/// compare, with the bad bytes replaced; an output file gone is empty.
#[test]
fn output_that_is_not_utf8_still_opens_and_searches() {
    let lab = lab(2);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let run = keep(&store, &ws, &plan, "ok\n");
    let kept = store.run(&ws, &run).unwrap();
    let folder = store.root().join(&ws).join(&run);
    fs::write(
        folder.join(&kept.hosts[0].file),
        b"caf\xe9 \xff\xfe needle\n",
    )
    .unwrap();
    fs::remove_file(folder.join(&kept.hosts[1].file)).unwrap();
    let first = OutputRef {
        run: run.clone(),
        host: kept.hosts[0].id.clone(),
    };
    let output = store.output(&ws, &first).unwrap();
    assert_eq!(output, "caf\u{fffd} \u{fffd}\u{fffd} needle\n");
    let hits = store.search(&ws, &run, "NEEDLE").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].count, 1);
    let second = OutputRef {
        run: run.clone(),
        host: kept.hosts[1].id.clone(),
    };
    assert_eq!(store.output(&ws, &second).unwrap(), "");
    assert!(!store.compare(&ws, &first, &second).unwrap().is_empty());
}

/// A run deleted while its tab or window shows it: everything asked of it
/// afterwards says there is no such run (what the run tab shows as gone),
/// nothing half of it stays listed, and deleting it again says the same.
/// A reader that has one of its files open does not stop the delete.
#[test]
fn a_run_deleted_while_open_is_gone_for_everything_asked_of_it() {
    let lab = lab(2);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let run = keep(&store, &ws, &plan, "r1#\n");
    let other = keep(&store, &ws, &plan, "r2#\n");
    let kept = store.run(&ws, &run).unwrap();
    let reading =
        fs::File::open(store.root().join(&ws).join(&run).join(&kept.hosts[0].file)).unwrap();

    store.delete(&ws, &run).unwrap();
    drop(reading);
    let gone = |result: Result<(), ResultsError>| match result {
        Err(ResultsError::NoSuchRun(id)) => assert_eq!(id, run),
        other => panic!("{other:?}"),
    };
    let reference = OutputRef {
        run: run.clone(),
        host: kept.hosts[0].id.clone(),
    };
    gone(store.run(&ws, &run).map(|_| ()));
    gone(store.output(&ws, &reference).map(|_| ()));
    gone(store.search(&ws, &run, "r1").map(|_| ()));
    gone(store.compare(&ws, &reference, &reference).map(|_| ()));
    gone(store.export(&ws, &run, &lab.folder("export")).map(|_| ()));
    gone(store.delete(&ws, &run));
    let listed = store.list(&ws).unwrap();
    assert_eq!(listed.runs.len(), 1);
    assert_eq!(listed.runs[0].id, other);
    assert!(listed.damaged.is_empty(), "{:?}", listed.damaged);
    assert!(!store.root().join(&ws).join(&run).exists());
}

/// While a run goes on, its description is written at most every
/// `SAVE_EVERY` (a run of 100 hosts spent most of its time rewriting it),
/// but never later than the next event after that, nor than the end, nor
/// than the recorder going away (the app closing).
#[test]
fn a_running_runs_description_is_written_now_and_then_and_at_the_end() {
    let lab = lab(2);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let mut recorder = store.begin(&ws, &folder(""), &plan).unwrap();
    let id = recorder.id().to_string();
    let state = |host: usize, state: HostState| JobEvent::State {
        host: plan.hosts[host].id.clone(),
        state,
    };
    let kept_states = || -> Vec<HostState> {
        store
            .run(&ws, &id)
            .unwrap()
            .hosts
            .into_iter()
            .map(|host| host.state)
            .collect()
    };
    // Just after begin: written, then nothing for a moment.
    recorder.record(&state(0, HostState::Connecting)).unwrap();
    // After the interval, the next event writes everything so far.
    std::thread::sleep(SAVE_EVERY + Duration::from_millis(50));
    recorder.record(&state(1, HostState::Connecting)).unwrap();
    assert_eq!(
        kept_states(),
        [HostState::Connecting, HostState::Connecting]
    );
    recorder.record(&state(0, HostState::Done)).unwrap();
    // `flush` writes at once.
    recorder.flush().unwrap();
    assert_eq!(kept_states(), [HostState::Done, HostState::Connecting]);
    recorder.record(&state(1, HostState::Running)).unwrap();
    // Dropped (the app closing without an end): the latest states stay.
    drop(recorder);
    assert_eq!(kept_states(), [HostState::Done, HostState::Running]);
    let run = store.run(&ws, &id).unwrap();
    assert!(!run.ended());
    assert!(store.running(&ws).is_empty());
}

/// How long `runs` kept runs of a workspace of ten hosts take to list,
/// and to list again.
struct Listing {
    first: Duration,
    again: Duration,
    /// To read every description whole, one after the other, as each list
    /// did before #137.
    reading: Duration,
}

fn list_many(runs: usize) -> Listing {
    let lab = lab(10);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let template = keep(&store, &ws, &plan, "r1# show hostname\nr1\nr1#\n");
    let from = store.root().join(&ws).join(&template);
    let text = fs::read_to_string(from.join(RUN_FILE)).unwrap();
    let making = Instant::now();
    for n in 1..runs {
        let id = format!("2001-01-01_00-00-00_{}", n + 1);
        let to = store.root().join(&ws).join(&id);
        fs::create_dir_all(to.join("output")).unwrap();
        fs::write(to.join(RUN_FILE), &text).unwrap();
    }
    // Kept runs are old: a description written a moment ago is read at
    // every list (its time cannot tell one write from the next).
    let old = SystemTime::now() - Duration::from_secs(3600);
    for entry in fs::read_dir(store.root().join(&ws)).unwrap() {
        let file = entry.unwrap().path().join(RUN_FILE);
        fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let made = making.elapsed();
    let listing = Instant::now();
    let listed = store.list(&ws).unwrap();
    let took = listing.elapsed();
    assert_eq!(listed.runs.len(), runs);
    assert!(listed.damaged.is_empty());
    let listing = Instant::now();
    let again = store.list(&ws).unwrap();
    let took_again = listing.elapsed();
    assert_eq!(again, listed);
    let reading = Instant::now();
    for entry in fs::read_dir(store.root().join(&ws)).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path().join(RUN_FILE)).unwrap();
        let run: StoredRun = serde_norway::from_str(&text).unwrap();
        assert_eq!(run.hosts.len(), 10);
    }
    let reading = reading.elapsed();
    println!(
        "{runs} kept runs of 10 hosts: made in {made:?}, listed in {took:?}, again in {took_again:?}; read one by one in {reading:?}"
    );
    Listing {
        first: took,
        again: took_again,
        reading,
    }
}

/// Two hundred kept runs list in well under a second's worth per hundred.
#[test]
fn two_hundred_kept_runs_list_quickly() {
    let took = list_many(200).first;
    assert!(took < Duration::from_secs(5), "{took:?}");
}

/// Listing again reads only the descriptions that changed (#137; every
/// list used to read every description whole, which the Jobs view waits
/// for each time a job starts or ends): a second list of 2 000 runs takes
/// well under half of reading every description, measured beside it. Not
/// timed on Windows: there, reading a file's size and time opens the file,
/// which on some computers (CI's runners, with their scanner) costs as much
/// as reading a description this small; what is not read again is checked
/// on every system by the next test.
#[test]
fn listing_many_kept_runs_again_is_quick() {
    let Listing {
        first,
        again,
        reading,
    } = list_many(2000);
    if cfg!(not(windows)) {
        assert!(
            again < reading / 2,
            "first {first:?}, again {again:?}, reading {reading:?}"
        );
    }
}

/// A settled description whose size and time have not changed is not read
/// again: changed to the same size with its time put back, the list still
/// gives what it held before; once its time changes, the new contents
/// (#137).
#[test]
fn a_settled_description_is_read_again_only_when_its_stamp_changes() {
    let lab = lab(2);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let id = keep(&store, &ws, &plan, "r1# show hostname\nr1\nr1#\n");
    let file = store.root().join(&ws).join(&id).join(RUN_FILE);
    let set_time = |time: SystemTime| {
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(time)
            .unwrap();
    };
    let old = SystemTime::now() - Duration::from_secs(3600);
    set_time(old);
    let listed = store.list(&ws).unwrap();
    assert_eq!(listed.runs[0].step, snippet("Show"));

    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("name: Show"), "{text}");
    fs::write(&file, text.replace("name: Show", "name: Shaw")).unwrap();
    set_time(old);
    assert_eq!(store.list(&ws).unwrap(), listed, "read again");

    set_time(old + Duration::from_secs(60));
    assert_eq!(store.list(&ws).unwrap().runs[0].step, snippet("Shaw"));
}

/// A description written again a moment after it was listed, with the same
/// size and the same file time, is read again (#137). A file clock is
/// coarse: on Linux a host's state going from `waiting` to `running`
/// within one tick left the list showing `waiting`.
#[test]
fn a_fresh_description_rewritten_with_the_same_size_and_time_is_read_again() {
    let lab = lab(2);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let run = keep(&store, &ws, &plan, "r1#\n");
    let file = store.root().join(&ws).join(&run).join(RUN_FILE);
    let first = store.list(&ws).unwrap();
    assert_eq!(first.runs[0].hosts[0].name, "r0");

    let written = fs::metadata(&file).unwrap().modified().unwrap();
    let text = fs::read_to_string(&file).unwrap();
    let renamed = text.replacen("name: r0", "name: q0", 1);
    assert_ne!(text, renamed);
    assert_eq!(text.len(), renamed.len());
    fs::write(&file, &renamed).unwrap();
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(written)
        .unwrap();
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), written);

    let second = store.list(&ws).unwrap();
    assert_eq!(second.runs[0].hosts[0].name, "q0");
}

/// What a store lists after its runs were listed once and then changed on
/// disk: a run rewritten (ended, a host's state changed), one added, one
/// deleted, one damaged, one repaired, through the store and behind its
/// back; another store on the same folder sees the same.
#[test]
fn a_second_listing_sees_every_change_since_the_first() {
    let lab = lab(2);
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let store = RunStore::new(lab.folder("data/jobs"));
    let ws = lab.id();
    let ended = keep(&store, &ws, &plan, "r1#\n");
    let gone = keep(&store, &ws, &plan, "r1#\n");
    let broken = keep(&store, &ws, &plan, "r1#\n");
    let mut running = store.begin(&ws, &folder(""), &plan).unwrap();
    let running_id = running.id().to_string();
    let first = store.list(&ws).unwrap();
    assert_eq!(first.runs.len(), 4);
    assert!(first.damaged.is_empty());
    let listed = |list: &RunList, id: &str| list.runs.iter().find(|run| run.id == id).cloned();
    assert!(!listed(&first, &running_id).unwrap().ended());

    // Through the store: the running run gets a state and its end.
    running
        .record(&JobEvent::State {
            host: plan.hosts[0].id.clone(),
            state: HostState::Running,
        })
        .unwrap();
    running.flush().unwrap();
    let second = store.list(&ws).unwrap();
    assert_eq!(
        listed(&second, &running_id).unwrap().hosts[0].state,
        HostState::Running
    );
    running
        .record(&JobEvent::Finished(report(&plan, "r1#\n")))
        .unwrap();
    drop(running);

    // Behind its back: one deleted, one damaged, one rewritten by hand
    // to the same size, one added.
    let root = store.root().join(&ws);
    fs::remove_dir_all(root.join(&gone)).unwrap();
    fs::write(root.join(&broken).join(RUN_FILE), "hosts: [").unwrap();
    let text = fs::read_to_string(root.join(&ended).join(RUN_FILE)).unwrap();
    let renamed = text.replacen("name: r0", "name: q0", 1);
    assert_ne!(text, renamed);
    assert_eq!(text.len(), renamed.len());
    fs::write(root.join(&ended).join(RUN_FILE), &renamed).unwrap();
    let added = "2001-01-01_00-00-00";
    fs::create_dir_all(root.join(added)).unwrap();
    fs::write(root.join(added).join(RUN_FILE), &text).unwrap();

    for store in [store.clone(), RunStore::new(store.root())] {
        let third = store.list(&ws).unwrap();
        assert!(listed(&third, &running_id).unwrap().ended());
        assert!(listed(&third, &gone).is_none());
        assert!(listed(&third, &broken).is_none());
        assert_eq!(
            third
                .damaged
                .iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>(),
            [broken.as_str()]
        );
        assert_eq!(listed(&third, &ended).unwrap().hosts[0].name, "q0");
        assert_eq!(listed(&third, added).unwrap().id, added);
        assert_eq!(third.runs.len(), 3);
    }

    // Repaired: listed again.
    fs::write(root.join(&broken).join(RUN_FILE), &text).unwrap();
    let fourth = store.list(&ws).unwrap();
    assert!(fourth.damaged.is_empty());
    assert!(listed(&fourth, &broken).is_some());
}

#[test]
#[ignore = "heavy: 5 000 run folders written to disk (a minute); `cargo test -p enodix-core --test edge_runs -- --ignored --nocapture`"]
fn five_thousand_kept_runs_list() {
    let took = list_many(5000).first;
    assert!(took < Duration::from_secs(30), "{took:?}");
}
