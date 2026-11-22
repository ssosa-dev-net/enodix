//! Edge tests (#117): large inventories. Every piece of work the app does
//! with the whole workspace (loading it, the tree the interface is sent, a
//! search, a saved filter, a live reload after one file changed, the
//! inventory export, planning a job on every host) must grow no worse than
//! linearly with the number of hosts, in a flat folder and in deep ones.
//!
//! The kept test compares 500 hosts with 4 000 in a flat folder (a linear
//! piece of work takes about eight times as long, a quadratic one
//! sixty-four), measuring the two in turn, twice, and keeping the best of
//! each piece. The measurements of 5 000 and 20 000 hosts are `#[ignore]`d,
//! as writing 20 000 files takes a while; run them in a release build:
//!
//! `cargo test --release -p enodix-core --test edge_inventory_scale -- --ignored --nocapture`
//!
//! Windows and Linux alike (the numbers differ: file creation and reading
//! cost more on Windows).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::inventory::{self, Limit};
use enodix_core::job::{self, JobStep, JobTarget};
use enodix_core::workspace::Workspace;
use tempfile::TempDir;

const SCOPE: &str = "edge-scale";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Every host in the workspace folder itself.
    Flat,
    /// Four levels of folders (region, site, room), forty hosts a room,
    /// each level with a settings file.
    Deep,
}

fn host_text(index: usize) -> String {
    format!(
        "address: 10.{}.{}.{}\nvendor: {}\nplatform: eos\nmodel: model-{}\nsite: site{:02}\nrole: {}\n\
         credential_profile: lab\ntags: [gen, rack{}]\ncustom:\n  circuit: CID-{index}\n  owner: team{}\n",
        index / 65536,
        (index / 256) % 256,
        index % 256,
        ["arista", "cisco", "juniper"][index % 3],
        index % 40,
        index % 30,
        ["core", "access", "edge"][index % 3],
        index % 10,
        index % 7,
    )
}

/// A workspace of `hosts` hosts, with a saved filter and a jump host every
/// host is reached through.
fn generated(hosts: usize, shape: Shape) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("_workspace.yaml"), format!("id: {SCOPE}\n")).unwrap();
    fs::write(
        root.join("_filters.yaml"),
        "Arista core:\n  vendor: arista\n  role: core\nTeam three:\n  custom:\n    owner: team3\n",
    )
    .unwrap();
    fs::write(
        root.join("bastion.yaml"),
        "address: 192.0.2.1\ncredential_profile: lab\n",
    )
    .unwrap();
    for index in 0..hosts {
        let folder = match shape {
            Shape::Flat => root.to_path_buf(),
            Shape::Deep => {
                let room = index / 40;
                root.join(format!("region{}", room / 100))
                    .join(format!("site{}", room / 10))
                    .join(format!("room{}", room))
            }
        };
        if !folder.exists() {
            fs::create_dir_all(&folder).unwrap();
            if shape == Shape::Deep {
                let room = index / 40;
                fs::write(
                    folder.join("_folder.yaml"),
                    format!("tags: [room{room}]\njump_host: bastion.yaml\n"),
                )
                .unwrap();
            }
        }
        let mut text = host_text(index);
        if shape == Shape::Flat {
            text.push_str("jump_host: bastion.yaml\n");
        }
        fs::write(folder.join(format!("host-{index:05}.yaml")), text).unwrap();
    }
    temp
}

/// How long each piece of work took, best of `runs`.
#[derive(Debug, Default, Clone)]
struct Times {
    open: Duration,
    tree: Duration,
    search: Duration,
    filter: Duration,
    refresh: Duration,
    export: Duration,
    plan: Duration,
}

impl Times {
    /// The better of the two for each piece.
    fn best_of(&self, other: &Times) -> Times {
        Times {
            open: self.open.min(other.open),
            tree: self.tree.min(other.tree),
            search: self.search.min(other.search),
            filter: self.filter.min(other.filter),
            refresh: self.refresh.min(other.refresh),
            export: self.export.min(other.export),
            plan: self.plan.min(other.plan),
        }
    }

    fn pieces(&self) -> [(&'static str, Duration); 7] {
        [
            ("open", self.open),
            ("tree", self.tree),
            ("search", self.search),
            ("filter", self.filter),
            ("refresh", self.refresh),
            ("export", self.export),
            ("plan", self.plan),
        ]
    }
}

fn best(runs: usize, mut work: impl FnMut()) -> Duration {
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            work();
            started.elapsed()
        })
        .min()
        .unwrap()
}

/// `runs` for the work in memory, `io_runs` for loading and reloading.
fn measure(root: &Path, hosts: usize, runs: usize, io_runs: usize) -> Times {
    let mut workspace = Workspace::open(root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    assert_eq!(workspace.search("", 1).total, hosts + 1);
    let store = MemoryStore::new();
    store
        .put(
            SCOPE,
            &CredentialProfile {
                name: "lab".into(),
                username: "admin".into(),
                auth: ProfileAuth::Password {
                    password: "pw".into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();

    let mut times = Times {
        open: best(io_runs, || {
            Workspace::open(root).unwrap();
        }),
        ..Times::default()
    };
    // What the interface is sent: the tree and what goes with each host.
    times.tree = best(runs, || {
        let tree = serde_json::to_vec(workspace.tree()).unwrap();
        let roles = workspace.host_roles();
        let safety = workspace.host_paste_safety();
        let names = workspace.platform_names();
        assert!(!tree.is_empty() && roles.len() == hosts && safety.len() == hosts + 1);
        assert!(!names.is_empty());
    });
    let queries = [
        "host-0",
        "10.0.1",
        "cid-77",
        "arista site07",
        "team6",
        "e",
        "zzz",
    ];
    times.search = best(runs, || {
        for query in queries {
            workspace.search(query, 50);
        }
    }) / queries.len() as u32;
    times.filter = best(runs, || {
        let filter = workspace.filter("Arista core").unwrap().clone();
        let shown = workspace.apply_filter(&filter);
        assert!(!shown.hosts.is_empty());
    });
    // A live reload after one host changed on disk.
    let changed = workspace
        .search("host-00000", 1)
        .hits
        .first()
        .map(|hit| root.join(hit.id.as_str()))
        .unwrap();
    times.refresh = best(io_runs, || {
        let had = fs::read_to_string(&changed)
            .unwrap()
            .contains("description");
        let mut text = host_text(0);
        if !had {
            text.push_str("description: changed\n");
        }
        fs::write(&changed, text).unwrap();
        assert!(workspace.refresh().unwrap());
    });
    times.export = best(runs, || {
        let export = inventory::export(&workspace, &Limit::All).unwrap();
        let json = serde_json::to_vec(&export).unwrap();
        assert_eq!(export.hosts.len(), hosts + 1);
        assert!(!json.is_empty());
    });
    times.plan = best(runs, || {
        let plan = job::plan(
            &workspace,
            &Profiles::new(&store, &workspace),
            &JobTarget::Folder {
                path: String::new(),
            },
            &JobStep::ConfigBackup,
        )
        .unwrap();
        assert_eq!(plan.hosts.len(), hosts, "{:?}", plan.skipped.first());
    });
    times
}

fn table(rows: &[(usize, Shape, Times)]) -> String {
    let mut out = String::from("hosts  shape ");
    for (name, _) in Times::default().pieces() {
        let _ = write!(out, "{name:>10}");
    }
    out.push('\n');
    for (hosts, shape, times) in rows {
        let _ = write!(
            out,
            "{hosts:>5}  {:<5} ",
            format!("{shape:?}").to_lowercase()
        );
        for (_, took) in times.pieces() {
            let _ = write!(out, "{:>8.1}ms", took.as_secs_f64() * 1000.0);
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_work_on_the_whole_inventory_grows_linearly_with_the_number_of_hosts() {
    // A flat folder is where a walk through siblings shows: eight times the
    // hosts must take about eight times as long (a quadratic piece of work
    // takes sixty-four: before #117, planning a job did, from 20 000 hosts
    // in one folder on).
    let (small_hosts, large_hosts) = (500, 4000);
    let small = generated(small_hosts, Shape::Flat);
    let large = generated(large_hosts, Shape::Flat);
    // In turn, so that a burst of load on the machine hits both alike.
    let mut small_times = measure(small.path(), small_hosts, 3, 1);
    let mut large_times = measure(large.path(), large_hosts, 3, 1);
    small_times = small_times.best_of(&measure(small.path(), small_hosts, 3, 1));
    large_times = large_times.best_of(&measure(large.path(), large_hosts, 3, 1));
    let rows = [
        (small_hosts, Shape::Flat, small_times.clone()),
        (large_hosts, Shape::Flat, large_times.clone()),
    ];
    println!("{}", table(&rows));
    for ((name, small), (_, large)) in small_times.pieces().into_iter().zip(large_times.pieces()) {
        // Below a few milliseconds the clock and a busy machine say more
        // than the work does.
        if large < Duration::from_millis(5) {
            continue;
        }
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
        // Linear is 8 and quadratic 64; other agents' builds on the same
        // machine have pushed a linear piece to 15.
        let most = 24.0;
        assert!(
            ratio < most,
            "{name} took {ratio:.1} times as long for eight times the hosts:\n{}",
            table(&rows)
        );
    }
}

/// What the shell holds its workspace lock for on a live reload of 20 000
/// hosts after one file changed (#132): the whole refresh (as before), or
/// only taking a workspace loaded beside the lock (`refresh_with`); and
/// what the shell then does under the lock either way, building the view
/// it sends (here: the tree, roles and paste safety, as JSON).
#[test]
#[ignore = "writes 20 000 files: run in a release build with --ignored --nocapture"]
fn a_refresh_of_a_large_inventory_holds_the_workspace_only_to_take_it() {
    use std::sync::Mutex;
    let hosts = 20_000;
    let temp = generated(hosts, Shape::Flat);
    let root = temp.path();
    let shared = Mutex::new(Workspace::open(root).unwrap());
    let change = |n: usize| {
        fs::write(
            root.join("host-00001.yaml"),
            format!("address: 10.9.9.{n}\n"),
        )
        .unwrap();
    };
    let mut under_lock = Vec::new();
    let mut beside = Vec::new();
    let mut view = Vec::new();
    for round in 0..3 {
        change(round * 2);
        let started = Instant::now();
        assert!(shared.lock().unwrap().refresh().unwrap());
        under_lock.push(started.elapsed());

        change(round * 2 + 1);
        let fresh = Workspace::open(root);
        let started = Instant::now();
        assert!(shared.lock().unwrap().refresh_with(fresh).unwrap());
        beside.push(started.elapsed());

        let started = Instant::now();
        let guard = shared.lock().unwrap();
        let json = serde_json::to_vec(&(
            guard.tree(),
            guard.roles(),
            guard.host_roles(),
            guard.host_paste_safety(),
        ))
        .unwrap();
        drop(guard);
        view.push(started.elapsed());
        assert!(!json.is_empty());
    }
    let best = |times: &[Duration]| times.iter().min().copied().unwrap();
    println!(
        "{hosts} hosts, the lock held for: the whole refresh {:?}, taking one loaded beside {:?}; \
         the view after it {:?}",
        best(&under_lock),
        best(&beside),
        best(&view)
    );
}

#[test]
#[ignore = "writes 50 000 files: run in a release build with --ignored --nocapture (see the module's note)"]
fn large_inventories_measured() {
    let mut rows = Vec::new();
    for hosts in [5000, 20000] {
        for shape in [Shape::Flat, Shape::Deep] {
            let started = Instant::now();
            let temp = generated(hosts, shape);
            println!("{hosts} {shape:?}: written in {:?}", started.elapsed());
            rows.push((hosts, shape, measure(temp.path(), hosts, 3, 2)));
            println!("{}", table(&rows));
        }
    }
}
