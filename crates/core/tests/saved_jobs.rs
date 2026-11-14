//! Saved jobs: loading the jobs file of the `jobs` fixture (good entries,
//! entries whose step or target is gone, bad entries), planning a saved job,
//! and saving, renaming and deleting them on a copy.

mod support;

use std::collections::BTreeMap;
use std::fs;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::job::{self, JobPlan, JobStep, JobTarget, PlanError};
use netterm_core::workspace::{EditError, HostId, SavedJob, ScriptError, Workspace, JOBS_FILE};
use support::workspaces::{fixture, fixture_copy, read};

fn open() -> Workspace {
    Workspace::open(fixture("jobs")).unwrap()
}

fn snippet(name: &str) -> JobStep {
    JobStep::Snippet {
        name: name.into(),
        answers: BTreeMap::new(),
    }
}

fn folder(path: &str) -> JobTarget {
    JobTarget::Folder { path: path.into() }
}

fn job(name: &str, step: JobStep, target: JobTarget) -> SavedJob {
    SavedJob {
        name: name.into(),
        step,
        target,
    }
}

/// A store holding the profile every host of the fixture logs in with.
fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: "admin".into(),
                auth: ProfileAuth::Password {
                    password: "secret".into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    store
}

/// The saved job `name` planned; its target is the saved job's.
fn plan_saved(workspace: &Workspace, name: &str) -> Result<JobPlan, PlanError> {
    let store = store(workspace);
    let (target, plan) = job::plan_saved(workspace, &Profiles::new(&store, workspace), name)?;
    assert_eq!(
        Some(&target),
        workspace.saved_job(name).map(|job| &job.target)
    );
    Ok(plan)
}

/// The hosts a plan covers, run or skipped, in target order.
fn covered(plan: &JobPlan) -> Vec<String> {
    let mut ids: Vec<String> = plan.hosts.iter().map(|h| h.id.to_string()).collect();
    ids.extend(plan.skipped.iter().map(|h| h.id.to_string()));
    ids
}

fn names(workspace: &Workspace) -> Vec<&str> {
    workspace
        .saved_jobs()
        .iter()
        .map(|job| job.name.as_str())
        .collect()
}

fn problems(workspace: &Workspace) -> Vec<String> {
    workspace
        .problems()
        .iter()
        .filter(|problem| problem.file == JOBS_FILE)
        .map(|problem| problem.message.clone())
        .collect()
}

// --- Loading ---

#[test]
fn saved_jobs_load_in_file_order_with_their_step_and_target() {
    let workspace = open();
    assert_eq!(
        names(&workspace),
        [
            "Madrid versions",
            "Core interfaces",
            "Check r1",
            "Everything",
            "Gone snippet",
            "Gone script",
            "Gone folder",
            "Gone filter",
            "Gone host",
        ]
    );
    let jobs = workspace.saved_jobs();
    assert_eq!(
        jobs[0],
        job("Madrid versions", snippet("Show version"), folder("Madrid"))
    );
    assert_eq!(
        jobs[1],
        job(
            "Core interfaces",
            JobStep::Snippet {
                name: "Interface".into(),
                answers: BTreeMap::from([("interface".into(), "Ethernet1".into())]),
            },
            JobTarget::Filter {
                name: "Core".into()
            },
        )
    );
    assert_eq!(
        jobs[2],
        job(
            "Check r1",
            JobStep::Script {
                path: "scripts/check.py".into()
            },
            JobTarget::Hosts {
                ids: vec![HostId::new("Madrid/r1.yaml")]
            },
        )
    );
    assert_eq!(jobs[3].target, folder(""));
    assert_eq!(
        workspace.saved_job("  madrid VERSIONS ").map(|j| &j.name),
        Some(&"Madrid versions".to_string())
    );
    assert!(workspace.saved_job("Nothing").is_none());
}

#[test]
fn bad_entries_are_problems_on_the_jobs_file_and_are_left_out() {
    let workspace = open();
    assert_eq!(
        problems(&workspace),
        [
            "saved job `Just text`: expected `snippet` or `script`, and `folder`, `filter` or `host`",
            "saved job `No step`: missing `snippet` or `script`",
            "saved job `No target`: missing `folder`, `filter` or `host`",
            "saved job `Two steps`: give `snippet` or `script`, not both",
            "saved job `Two targets`: give one of `folder`, `filter` and `host`, not several",
            "saved job `Answers for a script`: `answers` only go with a `snippet`",
            "saved job `Unknown key`: unknown key `when`",
            "saved job `Bad answers`: `answers` must map variable names to values",
            "saved job `madrid VERSIONS` is defined twice",
        ]
    );
    // The rest of the workspace loads as ever.
    assert_eq!(workspace.snippets().len(), 2);
    assert_eq!(workspace.filters().len(), 1);
}

#[test]
fn a_jobs_file_that_is_not_a_mapping_is_one_problem() {
    let (_temp, root) = fixture_copy("jobs");
    fs::write(root.join(JOBS_FILE), "- just\n- a list\n").unwrap();
    let workspace = Workspace::open(&root).unwrap();
    assert!(workspace.saved_jobs().is_empty());
    assert_eq!(problems(&workspace).len(), 1);
}

// --- What is gone, and planning ---

#[test]
fn a_saved_job_whose_step_or_target_is_gone_says_so() {
    let workspace = open();
    let problem = |name: &str| {
        job::saved_job_problem(&workspace, workspace.saved_job(name).unwrap())
            .map(|error| error.to_string())
    };
    for good in [
        "Madrid versions",
        "Core interfaces",
        "Check r1",
        "Everything",
    ] {
        assert_eq!(problem(good), None, "{good}");
    }
    assert_eq!(
        problem("Gone snippet").as_deref(),
        Some("there is no snippet called Show clock")
    );
    assert!(matches!(
        job::saved_job_problem(&workspace, workspace.saved_job("Gone script").unwrap()),
        Some(PlanError::NoSuchScript(ScriptError::NotFound(_)))
    ));
    assert_eq!(
        problem("Gone folder").as_deref(),
        Some("there is no folder \"Lisbon\" in the workspace")
    );
    assert_eq!(
        problem("Gone filter").as_deref(),
        Some("there is no saved filter called Edge")
    );
    assert_eq!(
        problem("Gone host").as_deref(),
        Some("Madrid/r9.yaml is no longer in the workspace")
    );

    // Planning refuses the same way, instead of running.
    for gone in [
        "Gone snippet",
        "Gone script",
        "Gone folder",
        "Gone filter",
        "Gone host",
    ] {
        assert_eq!(
            plan_saved(&workspace, gone).unwrap_err().to_string(),
            problem(gone).unwrap()
        );
    }
    assert_eq!(
        plan_saved(&workspace, "Nothing").unwrap_err(),
        PlanError::NoSuchJob("Nothing".into())
    );
}

#[test]
fn a_saved_job_plans_its_step_on_its_target_as_the_workspace_is_now() {
    let workspace = open();

    let plan = plan_saved(&workspace, "madrid versions").unwrap();
    assert_eq!(plan.step, snippet("Show version"));
    assert_eq!(covered(&plan), ["Madrid/r1.yaml", "Madrid/r2.yaml"]);
    assert!(plan.ready(), "{plan:?}");

    // A filter target: the hosts it shows, in tree order; the kept answers
    // fill the variable.
    let plan = plan_saved(&workspace, "Core interfaces").unwrap();
    assert_eq!(covered(&plan), ["Madrid/r1.yaml", "Paris/p1.yaml"]);
    assert!(plan.ready(), "{plan:?}");

    let plan = plan_saved(&workspace, "Check r1").unwrap();
    assert_eq!(covered(&plan), ["Madrid/r1.yaml"]);
    assert_eq!(plan.hosts.len(), 1);

    // Without answers, the variable is asked for, as for any snippet job.
    let plan = plan_saved(&workspace, "Everything").unwrap();
    assert_eq!(
        covered(&plan),
        ["Madrid/r1.yaml", "Madrid/r2.yaml", "Paris/p1.yaml"]
    );
    assert_eq!(plan.missing_variables, ["interface"]);
}

#[test]
fn a_filter_target_follows_the_filter_and_must_exist() {
    let (_temp, root) = fixture_copy("jobs");
    let mut workspace = Workspace::open(&root).unwrap();
    let store = MemoryStore::new();
    let core = JobTarget::Filter {
        name: "core".into(),
    };
    let plan = |workspace: &Workspace, target: &JobTarget| {
        job::plan(
            workspace,
            &Profiles::new(&store, workspace),
            target,
            &snippet("Show version"),
        )
    };
    assert_eq!(
        covered(&plan(&workspace, &core).unwrap()),
        ["Madrid/r1.yaml", "Paris/p1.yaml"]
    );
    fs::write(
        root.join("Madrid/r2.yaml"),
        "address: 10.0.0.2\ntags: [core]\n",
    )
    .unwrap();
    assert!(workspace.refresh().unwrap());
    assert_eq!(
        covered(&plan(&workspace, &core).unwrap()),
        ["Madrid/r1.yaml", "Madrid/r2.yaml", "Paris/p1.yaml"]
    );
    assert_eq!(
        plan(
            &workspace,
            &JobTarget::Filter {
                name: "Edge".into()
            }
        )
        .unwrap_err(),
        PlanError::NoSuchFilter("Edge".into())
    );
}

// --- Saving, renaming, deleting ---

#[test]
fn a_new_saved_job_goes_after_the_others_and_reloads() {
    let (_temp, root) = fixture_copy("jobs");
    let mut workspace = Workspace::open(&root).unwrap();
    let before = workspace.revision();
    workspace
        .save_job(
            None,
            &job(
                "  Nightly  ",
                JobStep::Snippet {
                    name: " Interface ".into(),
                    answers: BTreeMap::from([("interface".into(), "Ethernet2".into())]),
                },
                folder("/Paris/"),
            ),
        )
        .unwrap();
    assert!(workspace.revision() > before);
    assert_eq!(
        workspace.saved_job("nightly").unwrap(),
        &job(
            "Nightly",
            JobStep::Snippet {
                name: "Interface".into(),
                answers: BTreeMap::from([("interface".into(), "Ethernet2".into())]),
            },
            folder("Paris"),
        )
    );
    assert_eq!(names(&workspace).last(), Some(&"Nightly"));
    let text = read(&root, JOBS_FILE);
    assert!(
        text.ends_with(
            "Nightly:\n  snippet: Interface\n  answers:\n    interface: Ethernet2\n  folder: Paris\n"
        ),
        "{text}"
    );
    // Bad entries stay in the file, still problems.
    assert!(text.contains("Unknown key:"), "{text}");
    assert_eq!(problems(&workspace).len(), 9);

    workspace
        .save_job(
            None,
            &job(
                "Audit",
                JobStep::Script {
                    path: "scripts\\check.py".into(),
                },
                JobTarget::Filter {
                    name: "Core".into(),
                },
            ),
        )
        .unwrap();
    workspace
        .save_job(
            None,
            &job(
                "One",
                snippet("Show version"),
                JobTarget::Hosts {
                    ids: vec![HostId::new("Paris/p1.yaml")],
                },
            ),
        )
        .unwrap();
    workspace
        .save_job(None, &job("Root", snippet("Show version"), folder("")))
        .unwrap();
    let text = read(&root, JOBS_FILE);
    assert!(
        text.ends_with(concat!(
            "Audit:\n  script: scripts/check.py\n  filter: Core\n",
            "One:\n  snippet: Show version\n  host: Paris/p1.yaml\n",
            "Root:\n  snippet: Show version\n  folder: ''\n",
        )),
        "{text}"
    );
    // Read back as saved.
    let reopened = Workspace::open(&root).unwrap();
    assert_eq!(reopened.saved_jobs(), workspace.saved_jobs());
}

#[test]
fn a_saved_job_is_renamed_and_changed_in_place() {
    let (_temp, root) = fixture_copy("jobs");
    let mut workspace = Workspace::open(&root).unwrap();
    workspace
        .save_job(
            Some("madrid versions"),
            &job("Madrid check", snippet("Show version"), folder("Paris")),
        )
        .unwrap();
    assert_eq!(&names(&workspace)[..2], ["Madrid check", "Core interfaces"]);
    assert!(workspace.saved_job("Madrid versions").is_none());
    let text = read(&root, JOBS_FILE);
    assert!(
        text.contains("Madrid check:\n  snippet: Show version\n  folder: Paris\nCore interfaces:"),
        "{text}"
    );
    // A change of case only is a rename too.
    workspace
        .save_job(
            Some("Madrid check"),
            &job("MADRID check", snippet("Show version"), folder("Paris")),
        )
        .unwrap();
    assert_eq!(names(&workspace)[0], "MADRID check");
}

#[test]
fn saving_refuses_clashes_backups_host_lists_and_blank_names() {
    let (_temp, root) = fixture_copy("jobs");
    let mut workspace = Workspace::open(&root).unwrap();
    let original = read(&root, JOBS_FILE);
    let refused = |workspace: &mut Workspace, previous: Option<&str>, job: SavedJob| {
        workspace.save_job(previous, &job).unwrap_err()
    };

    assert!(matches!(
        refused(
            &mut workspace,
            None,
            job("check R1", snippet("Show version"), folder("Madrid"))
        ),
        EditError::AlreadyExists(_)
    ));
    assert!(matches!(
        refused(
            &mut workspace,
            Some("Check r1"),
            job("Everything", snippet("Show version"), folder("Madrid"))
        ),
        EditError::AlreadyExists(_)
    ));
    // An entry defined badly is never written over.
    assert!(matches!(
        refused(
            &mut workspace,
            None,
            job("unknown KEY", snippet("Show version"), folder("Madrid"))
        ),
        EditError::AlreadyExists(_)
    ));
    assert!(matches!(
        refused(
            &mut workspace,
            Some("Nothing"),
            job("New", snippet("Show version"), folder("Madrid"))
        ),
        EditError::NotFound(_)
    ));
    for (bad, message) in [
        (
            job("Backup", JobStep::ConfigBackup, folder("Madrid")),
            "the job cannot be saved: a config backup cannot be saved as a job",
        ),
        (
            job(
                "Two",
                snippet("Show version"),
                JobTarget::Hosts {
                    ids: vec![
                        HostId::new("Madrid/r1.yaml"),
                        HostId::new("Madrid/r2.yaml"),
                    ],
                },
            ),
            "the job cannot be saved: a saved job runs on a folder, a saved filter or one host, not a list of hosts",
        ),
        (
            job(" ", snippet("Show version"), folder("Madrid")),
            "the job cannot be saved: it has no name",
        ),
        (
            job("Blank", snippet(" "), folder("Madrid")),
            "the job cannot be saved: it names no snippet",
        ),
    ] {
        assert_eq!(
            refused(&mut workspace, None, bad).to_string(),
            message
        );
    }
    assert_eq!(read(&root, JOBS_FILE), original, "no refused edit writes");
}

#[test]
fn deleting_saved_jobs_and_the_file_goes_with_the_last() {
    let (_temp, root) = fixture_copy("jobs");
    let mut workspace = Workspace::open(&root).unwrap();
    workspace.delete_job("GONE HOST").unwrap();
    assert!(workspace.saved_job("Gone host").is_none());
    assert!(!read(&root, JOBS_FILE).contains("Gone host:"));
    assert!(matches!(
        workspace.delete_job("Gone host"),
        Err(EditError::NotFound(_))
    ));

    fs::remove_file(root.join(JOBS_FILE)).unwrap();
    workspace.refresh().unwrap();
    workspace
        .save_job(
            None,
            &job("Only", snippet("Show version"), folder("Madrid")),
        )
        .unwrap();
    assert_eq!(
        read(&root, JOBS_FILE),
        "Only:\n  snippet: Show version\n  folder: Madrid\n"
    );
    workspace.delete_job("only").unwrap();
    assert!(!root.join(JOBS_FILE).exists());
    assert!(workspace.saved_jobs().is_empty());
}

#[test]
fn an_external_edit_of_the_jobs_file_is_picked_up_by_a_refresh() {
    let (_temp, root) = fixture_copy("jobs");
    let mut workspace = Workspace::open(&root).unwrap();
    assert!(!workspace.refresh().unwrap(), "nothing changed yet");
    fs::write(
        root.join(JOBS_FILE),
        "Pulled:\n  snippet: Show version\n  filter: Core\nBroken: {}\n",
    )
    .unwrap();
    assert!(workspace.refresh().unwrap());
    assert_eq!(names(&workspace), ["Pulled"]);
    assert_eq!(
        problems(&workspace),
        ["saved job `Broken`: missing `snippet` or `script`"]
    );
}
