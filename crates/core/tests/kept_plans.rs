//! The plans on screen (#121): each pre-flight's plan is kept under an id
//! of its own, so that a Start runs exactly the plan its tab showed, and a
//! plan with production hosts starts only when the start says they were
//! confirmed for that plan. Nothing here connects to anything.

use std::fs;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::job::{
    self, JobStep, JobTarget, KeptPlan, KeptPlans, PlanRefused, MAX_KEPT_PLANS,
};
use netterm_core::workspace::{HostId, Workspace};
use tempfile::TempDir;

/// A workspace with a Lab folder (two hosts) and a Production folder (two
/// production hosts, one named like a lab host), and a snippet.
fn workspace() -> (TempDir, Workspace, MemoryStore) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("plans");
    let host = |extra: &str| format!("address: 127.0.0.1\nplatform: eos\n{extra}");
    for (path, text) in [
        ("_folder.yaml", "credential_profile: lab\n".to_string()),
        ("Lab/l1.yaml", host("")),
        ("Lab/l2.yaml", host("")),
        (
            "Production/_folder.yaml",
            "environment: production\n".into(),
        ),
        ("Production/p1.yaml", host("")),
        ("Production/p2.yaml", host("")),
        ("_snippets.yaml", "Reload: reload\n".into()),
    ] {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: "engineer".into(),
                auth: ProfileAuth::Password {
                    password: "secret".into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    (temp, workspace, store)
}

fn kept(workspace: &Workspace, store: &MemoryStore, folder: &str) -> KeptPlan {
    let target = JobTarget::Folder {
        path: folder.into(),
    };
    let plan = job::plan(
        workspace,
        &Profiles::new(store, workspace),
        &target,
        &JobStep::Snippet {
            name: "Reload".into(),
            answers: Default::default(),
        },
    )
    .unwrap();
    KeptPlan {
        workspace: workspace.id().map(str::to_string),
        target,
        plan,
    }
}

fn ids(kept: &KeptPlan) -> Vec<&str> {
    kept.plan
        .hosts
        .iter()
        .map(|host| host.id.as_str())
        .collect()
}

fn production() -> Vec<HostId> {
    vec![
        HostId::new("Production/p1.yaml"),
        HostId::new("Production/p2.yaml"),
    ]
}

/// Two tabs at pre-flight: each start finds its own plan, whichever was
/// made last, and starting one leaves the other kept.
#[test]
fn each_plan_is_kept_under_its_own_id() {
    let (_temp, workspace, store) = workspace();
    let mut plans = KeptPlans::new();
    let production_plan = plans.keep(kept(&workspace, &store, "Production"));
    let lab_plan = plans.keep(kept(&workspace, &store, "Lab"));
    assert_ne!(production_plan, lab_plan);
    assert_eq!(plans.len(), 2);

    let lab = plans.ready(lab_plan, &[]).unwrap();
    assert_eq!(ids(lab), ["Lab/l1.yaml", "Lab/l2.yaml"]);
    let prod = plans.ready(production_plan, &production()).unwrap();
    assert_eq!(ids(prod), ["Production/p1.yaml", "Production/p2.yaml"]);

    // The lab job started: its plan is used up, the other stays.
    assert!(plans.forget(lab_plan));
    assert_eq!(plans.ready(lab_plan, &[]).unwrap_err(), PlanRefused::Gone);
    assert!(!plans.forget(lab_plan));
    assert!(plans.ready(production_plan, &production()).is_ok());
}

/// A plan with production hosts starts only if the start names every one
/// of them as confirmed; a confirmation for another plan's hosts does not
/// count, and the refusal names the hosts left.
#[test]
fn production_hosts_must_be_confirmed_for_the_plan_started() {
    let (_temp, workspace, store) = workspace();
    let mut plans = KeptPlans::new();
    let id = plans.keep(kept(&workspace, &store, "Production"));

    let refused = plans.ready(id, &[]).unwrap_err();
    assert_eq!(
        refused,
        PlanRefused::ProductionNotConfirmed(vec!["p1".into(), "p2".into()])
    );
    assert_eq!(
        refused.to_string(),
        "the production hosts p1, p2 were not confirmed for this job: confirm them in its pre-flight"
    );
    let partly = plans
        .ready(id, &[HostId::new("Production/p1.yaml")])
        .unwrap_err();
    assert_eq!(
        partly.to_string(),
        "the production host p2 was not confirmed for this job: confirm it in its pre-flight"
    );
    // Another plan's (lab) hosts confirm nothing here.
    assert!(plans
        .ready(
            id,
            &[HostId::new("Lab/l1.yaml"), HostId::new("Lab/l2.yaml")]
        )
        .is_err());
    assert!(plans.ready(id, &production()).is_ok());
    // Refusals keep the plan.
    assert_eq!(plans.len(), 1);
}

/// At most `MAX_KEPT_PLANS` plans are kept: making one more forgets the
/// oldest. Opening another workspace forgets them all. Ids are never
/// reused.
#[test]
fn the_kept_plans_are_bounded_and_cleared() {
    let (_temp, workspace, store) = workspace();
    let mut plans = KeptPlans::new();
    let first = plans.keep(kept(&workspace, &store, "Lab"));
    let second = plans.keep(kept(&workspace, &store, "Lab"));
    for _ in 2..MAX_KEPT_PLANS {
        plans.keep(kept(&workspace, &store, "Lab"));
    }
    assert_eq!(plans.len(), MAX_KEPT_PLANS);
    assert!(plans.ready(first, &[]).is_ok());
    let newest = plans.keep(kept(&workspace, &store, "Lab"));
    assert_eq!(plans.len(), MAX_KEPT_PLANS);
    assert_eq!(plans.ready(first, &[]).unwrap_err(), PlanRefused::Gone);
    assert!(plans.ready(second, &[]).is_ok());
    assert!(PlanRefused::Gone.to_string().contains("plan the job again"));

    plans.clear();
    assert!(plans.is_empty());
    assert_eq!(plans.ready(newest, &[]).unwrap_err(), PlanRefused::Gone);
    let after = plans.keep(kept(&workspace, &store, "Lab"));
    assert!(after > newest, "ids are not reused");
}
