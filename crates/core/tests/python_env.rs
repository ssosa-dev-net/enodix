//! The Python environment (#51): the stored choice, the interpreter checks
//! and their messages, finding a Python for the app's environment, the setup
//! plan, and (only when this machine has Python 3.10+ and Node) creating the
//! environment for real.

mod support;

use std::fs;
use std::time::{Duration, Instant};

use enodix_core::python::{
    self, create_environment, environment_folder, find_python, package_version, status, wheel_name,
    Candidate, FindError, InstallSource, Library, OfferError, PythonChoice, PythonProblem,
    PythonStatus, Setup, SetupCanceller, SetupError, SetupOffer, SetupProgress,
};
use support::processes;
use support::python::{build_wheel, real_python, too_old, FakePython};
use tempfile::TempDir;

const EXPECTED: &str = "0.1.2";

fn own(path: &std::path::Path) -> PythonChoice {
    PythonChoice {
        interpreter: path.to_path_buf(),
        setup: Setup::Own,
    }
}

#[test]
fn without_a_choice_python_features_are_off_and_say_how_to_turn_them_on() {
    let data = TempDir::new().unwrap();
    assert_eq!(PythonChoice::load(data.path()), None);
    let status = status(None, EXPECTED, None);
    assert!(!status.ready());
    let PythonStatus::NotSet { message } = &status else {
        panic!("{status:?}");
    };
    assert!(message.contains("Preferences"), "{message}");
    assert!(message.contains("own Python environment"), "{message}");
    assert!(message.contains("interpreter of your own"), "{message}");
}

#[test]
fn the_choice_is_kept_in_the_data_folder_and_can_be_forgotten() {
    let data = TempDir::new().unwrap();
    let choice = PythonChoice {
        interpreter: data.path().join("python").join("python.exe"),
        setup: Setup::Managed,
    };
    choice.save(data.path()).unwrap();
    // A later start reads it back.
    assert_eq!(PythonChoice::load(data.path()), Some(choice.clone()));
    let text = fs::read_to_string(data.path().join("python.yaml")).unwrap();
    assert!(text.contains("setup: managed"), "{text}");

    let mine = own(&data.path().join("mine.exe"));
    mine.save(data.path()).unwrap();
    assert_eq!(PythonChoice::load(data.path()), Some(mine));

    PythonChoice::clear(data.path()).unwrap();
    assert_eq!(PythonChoice::load(data.path()), None);
    // Forgetting twice is fine.
    PythonChoice::clear(data.path()).unwrap();
}

#[test]
fn a_damaged_choice_file_counts_as_no_choice() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("python.yaml"),
        "interpreter: [not, a, path\n",
    )
    .unwrap();
    assert_eq!(PythonChoice::load(data.path()), None);
    fs::write(data.path().join("python.yaml"), "setup: own\n").unwrap();
    assert_eq!(PythonChoice::load(data.path()), None);
}

#[test]
fn a_missing_interpreter_is_reported_as_such() {
    let data = TempDir::new().unwrap();
    let gone = data.path().join("nowhere").join("python.exe");
    let checked = status(Some(&own(&gone)), EXPECTED, None);
    let PythonStatus::Problem {
        problem, message, ..
    } = &checked
    else {
        panic!("{checked:?}");
    };
    assert_eq!(problem, &PythonProblem::Missing);
    assert!(message.contains("No Python at"), "{message}");
    assert!(message.contains("does not exist"), "{message}");

    let managed = PythonChoice {
        interpreter: gone,
        setup: Setup::Managed,
    };
    let checked = status(Some(&managed), EXPECTED, None);
    assert!(
        checked.message().contains("environment is gone"),
        "{}",
        checked.message()
    );
}

#[test]
fn a_too_old_python_is_reported_with_its_version() {
    let folder = TempDir::new().unwrap();
    let old = FakePython::answering(folder.path(), "old", "3.9.1", Some(EXPECTED));
    let checked = status(Some(&own(&old.path)), EXPECTED, None);
    let PythonStatus::Problem {
        problem, message, ..
    } = &checked
    else {
        panic!("{checked:?}");
    };
    assert_eq!(problem, &PythonProblem::TooOld { version: too_old() });
    assert!(message.contains("Python 3.9.1"), "{message}");
    assert!(message.contains("3.10 or newer"), "{message}");
}

#[test]
fn something_that_is_not_python_is_reported() {
    let folder = TempDir::new().unwrap();
    let other = FakePython::failing(folder.path(), "other");
    let checked = status(Some(&own(&other.path)), EXPECTED, None);
    let PythonStatus::Problem {
        problem, message, ..
    } = &checked
    else {
        panic!("{checked:?}");
    };
    assert!(
        matches!(problem, PythonProblem::NotPython { .. }),
        "{problem:?}"
    );
    assert!(message.contains("cannot be used as Python"), "{message}");
    assert!(message.contains("this is not python"), "{message}");
}

#[test]
fn a_python_without_the_package_says_how_to_install_it_from_the_wheel() {
    let folder = TempDir::new().unwrap();
    let bare = FakePython::answering(folder.path(), "bare", "3.12.4", None);
    let wheel = folder.path().join(wheel_name(EXPECTED));
    let checked = status(Some(&own(&bare.path)), EXPECTED, Some(&wheel));
    let PythonStatus::Problem {
        problem, message, ..
    } = &checked
    else {
        panic!("{checked:?}");
    };
    assert!(
        matches!(
            problem,
            PythonProblem::PackageMissing {
                reason: Some(_),
                ..
            }
        ),
        "{problem:?}"
    );
    assert!(
        message.contains("cannot import the app's package"),
        "{message}"
    );
    assert!(message.contains("-m pip install"), "{message}");
    assert!(message.contains(&wheel.display().to_string()), "{message}");
}

#[test]
fn a_package_of_another_version_is_reported() {
    let folder = TempDir::new().unwrap();
    let stale = FakePython::answering(folder.path(), "stale", "3.11.0", Some("0.0.9"));
    let checked = status(Some(&own(&stale.path)), EXPECTED, None);
    let PythonStatus::Problem { problem, .. } = &checked else {
        panic!("{checked:?}");
    };
    assert_eq!(
        problem,
        &PythonProblem::PackageMismatch {
            version: python::PythonVersion {
                major: 3,
                minor: 11,
                patch: 0
            },
            installed: "0.0.9".into(),
            expected: EXPECTED.into(),
        }
    );
    assert!(checked.message().contains("0.0.9"));
}

#[test]
fn a_working_interpreter_with_the_package_is_ready() {
    let folder = TempDir::new().unwrap();
    let good = FakePython::answering(folder.path(), "good", "3.12.4", Some(EXPECTED));
    let checked = status(Some(&own(&good.path)), EXPECTED, None);
    let PythonStatus::Ready {
        version,
        package,
        setup,
        ..
    } = &checked
    else {
        panic!("{checked:?}");
    };
    assert_eq!(version.to_string(), "3.12.4");
    assert_eq!(package, EXPECTED);
    assert_eq!(*setup, Setup::Own);
}

#[test]
fn package_versions_follow_the_wheel_build() {
    assert_eq!(package_version("0.1.2"), "0.1.2");
    assert_eq!(package_version("1.2.3-beta.1"), "1.2.3b1");
    assert_eq!(package_version("1.2.3-alpha.4"), "1.2.3a4");
    assert_eq!(package_version("1.2.3-rc.2+build.5"), "1.2.3rc2");
    assert_eq!(wheel_name("1.0.0"), "enodix-1.0.0-py3-none-any.whl");
}

#[test]
fn finding_a_python_takes_the_first_new_enough_one() {
    let folder = TempDir::new().unwrap();
    let missing = Candidate::new(folder.path().join("missing.exe"));
    let old = FakePython::answering(folder.path(), "old", "3.9.1", None);
    let new = FakePython::answering(folder.path(), "new", "3.13.0", None);
    let found = find_python(&[missing, old.candidate(), new.candidate()]).unwrap();
    assert_eq!(found.version.to_string(), "3.13.0");
    assert_eq!(found.executable, new.path);
}

#[test]
fn finding_no_python_or_only_old_ones_says_which() {
    let folder = TempDir::new().unwrap();
    let missing = Candidate::new(folder.path().join("missing.exe"));
    let error = find_python(&[missing.clone(), Candidate::new("no-such-python-here")]).unwrap_err();
    assert!(matches!(error, FindError::NoPython { .. }), "{error:?}");
    let message = error.to_string();
    assert!(message.starts_with("No Python was found"), "{message}");
    assert!(message.contains("no-such-python-here"), "{message}");

    let old = FakePython::answering(folder.path(), "old", "3.8.10", None);
    let error = find_python(&[missing, old.candidate()]).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("Only Python 3.8.10"), "{message}");
    assert!(message.contains("3.10 or newer is needed"), "{message}");
}

#[test]
fn the_offer_needs_the_wheel_shipped_with_the_app() {
    let folder = TempDir::new().unwrap();
    let data = folder.path().join("data");
    let good = FakePython::answering(folder.path(), "good", "3.12.4", None);
    let error = SetupOffer::find(
        &[good.candidate()],
        &environment_folder(&data),
        folder.path(),
        EXPECTED,
    )
    .unwrap_err();
    assert!(matches!(error, OfferError::NoWheel { .. }), "{error:?}");
    assert!(error.to_string().contains(&wheel_name(EXPECTED)));
}

#[test]
fn the_plan_lists_everything_and_installs_nothing() {
    let folder = TempDir::new().unwrap();
    let data = folder.path().join("data");
    let wheel = folder.path().join(wheel_name(EXPECTED));
    fs::write(&wheel, "not really a wheel").unwrap();
    let good = FakePython::answering(folder.path(), "good", "3.12.4", None);

    let offer = SetupOffer::find(
        &[good.candidate()],
        &environment_folder(&data),
        folder.path(),
        EXPECTED,
    )
    .unwrap();
    assert_eq!(offer.base.executable, good.path);
    assert_eq!(offer.environment, data.join("python"));
    assert!(!offer.existing);
    let offered: Vec<Library> = offer.libraries.iter().map(|l| l.library).collect();
    assert_eq!(offered, Library::ALL);

    // Only the app's package, from its wheel.
    let plan = offer.plan(&[]);
    assert_eq!(plan.installs.len(), 1);
    assert_eq!(plan.installs[0].name, wheel_name(EXPECTED));
    assert_eq!(
        plan.installs[0].source,
        InstallSource::Wheel {
            path: wheel.clone()
        }
    );

    // Ticked libraries from the index, Nornir's plugins for the others.
    let plan = offer.plan(&[Library::Netmiko, Library::Nornir, Library::Netmiko]);
    let names: Vec<&str> = plan.installs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        [
            wheel_name(EXPECTED).as_str(),
            "nornir>=3.3,<4",
            "netmiko",
            "nornir-netmiko"
        ]
    );
    assert!(plan.installs[1..]
        .iter()
        .all(|install| install.source == InstallSource::Index));
    assert_eq!(plan.libraries, [Library::Nornir, Library::Netmiko]);

    // Without Nornir, no plugins.
    let plan = offer.plan(&[Library::Scrapli, Library::Napalm]);
    let names: Vec<&str> = plan.installs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names[1..], ["scrapli", "napalm"]);

    // Nothing was created, nothing chosen.
    assert!(!data.exists());
    assert_eq!(PythonChoice::load(&data), None);
}

#[test]
fn a_failing_step_stops_the_setup_naming_it_and_chooses_nothing() {
    let folder = TempDir::new().unwrap();
    let data = folder.path().join("data");
    let wheel = folder.path().join(wheel_name(EXPECTED));
    fs::write(&wheel, "x").unwrap();
    let good = FakePython::answering(folder.path(), "good", "3.12.4", None);
    let mut offer = SetupOffer::find(
        &[good.candidate()],
        &environment_folder(&data),
        folder.path(),
        EXPECTED,
    )
    .unwrap();
    offer.base.executable = FakePython::failing(folder.path(), "broken").path;

    let mut heard = Vec::new();
    let error = create_environment(
        &offer.plan(&[Library::Nornir]),
        &data,
        EXPECTED,
        &SetupCanceller::new(),
        &mut |progress| heard.push(progress),
    )
    .unwrap_err();
    let SetupError::Failed { step, reason } = &error else {
        panic!("{error:?}");
    };
    assert!(step.starts_with("creating the environment"), "{step}");
    assert!(reason.contains("exit status 3"), "{reason}");
    assert!(reason.contains("this is not python"), "{reason}");
    assert!(
        matches!(&heard[0], SetupProgress::Step { description } if description.starts_with("Creating"))
    );
    assert!(heard.contains(&SetupProgress::Output {
        line: "this is not python".into()
    }));
    assert_eq!(PythonChoice::load(&data), None);
}

#[test]
fn a_setup_can_be_cancelled() {
    let folder = TempDir::new().unwrap();
    let data = folder.path().join("data");
    let wheel = folder.path().join(wheel_name(EXPECTED));
    fs::write(&wheel, "x").unwrap();
    let good = FakePython::answering(folder.path(), "good", "3.12.4", None);
    let mut offer = SetupOffer::find(
        &[good.candidate()],
        &environment_folder(&data),
        folder.path(),
        EXPECTED,
    )
    .unwrap();
    offer.base.executable = FakePython::sleeping(folder.path(), "slow").path;

    let canceller = SetupCanceller::new();
    let later = canceller.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        later.cancel();
    });
    let started = Instant::now();
    let error =
        create_environment(&offer.plan(&[]), &data, EXPECTED, &canceller, &mut |_| {}).unwrap_err();
    assert!(matches!(error, SetupError::Cancelled { .. }), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(PythonChoice::load(&data), None);
}

/// This test binary run again as a helper process (`support::processes`):
/// a tree of processes, or [`SETUP_OWNER`].
#[test]
fn helper() {
    processes::play();
    if processes::role().as_deref() == Some(SETUP_OWNER) {
        own_a_setup();
    }
}

/// A process that runs a setup whose first step is a process tree, and
/// then goes away without cancelling it (killed by the test).
const SETUP_OWNER: &str = "setup owner";

/// A setup plan in `folder` whose first step (creating the environment)
/// runs a process tree writing to `pids`.
fn tree_setup(folder: &std::path::Path, pids: &std::path::Path) -> python::SetupPlan {
    let data = folder.join("data");
    let wheel = folder.join(wheel_name(EXPECTED));
    fs::write(&wheel, "x").unwrap();
    let good = FakePython::answering(folder, "good", "3.12.4", None);
    let mut offer = SetupOffer::find(
        &[good.candidate()],
        &environment_folder(&data),
        folder,
        EXPECTED,
    )
    .unwrap();
    offer.base.executable = processes::tree_python(folder, "tree", pids).path;
    offer.plan(&[])
}

fn tree_pids(pids: &std::path::Path) -> Vec<u32> {
    processes::wait_for_pids(
        pids,
        &[processes::CHILD, processes::GRANDCHILD],
        Duration::from_secs(60),
    )
    .expect("the step started its child and grandchild")
}

/// Closing the app while a setup runs (#124): cancelled, it ends the
/// step's processes with what they started, and the app can wait for it.
#[test]
fn closing_the_app_stops_a_setup_and_the_processes_of_its_step() {
    let folder = TempDir::new().unwrap();
    let pids = folder.path().join("pids");
    fs::create_dir(&pids).unwrap();
    let plan = tree_setup(folder.path(), &pids);
    let data = folder.path().join("data");
    let canceller = SetupCanceller::new();
    let running = canceller.clone();
    let setup = std::thread::spawn(move || {
        create_environment(&plan, &data, EXPECTED, &running, &mut |_| {})
    });
    let started = tree_pids(&pids);
    assert!(!canceller.wait(Duration::ZERO), "the setup is running");
    canceller.cancel();
    assert!(canceller.wait(Duration::from_secs(30)), "the setup stopped");
    let error = setup.join().unwrap().unwrap_err();
    assert!(matches!(error, SetupError::Cancelled { .. }), "{error:?}");
    let alive = processes::wait_gone(&started, Duration::from_secs(30));
    assert!(alive.is_empty(), "still running: {alive:?}");
}

/// The app is killed, or crashes, during a setup (#124): the step's
/// processes end with it. The owner is this binary run again.
#[test]
fn a_setups_processes_end_when_the_app_is_killed() {
    let folder = TempDir::new().unwrap();
    let pids = folder.path().join("pids");
    fs::create_dir(&pids).unwrap();
    let mut owner = processes::helper(SETUP_OWNER, &pids)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let found = processes::wait_for_pids(
        &pids,
        &[processes::CHILD, processes::GRANDCHILD],
        Duration::from_secs(60),
    );
    let Some(started) = found else {
        let _ = owner.kill();
        panic!("the owner's setup did not start: {:?}", owner.wait());
    };
    assert!(owner.try_wait().unwrap().is_none(), "the owner went away");
    owner.kill().unwrap();
    owner.wait().unwrap();
    let alive = processes::wait_gone(&started, Duration::from_secs(30));
    assert!(alive.is_empty(), "still running: {alive:?}");
}

/// [`SETUP_OWNER`]: runs the setup and never cancels it.
fn own_a_setup() -> ! {
    let pids = std::path::PathBuf::from(std::env::var_os(processes::PIDS_VARIABLE).unwrap());
    let folder = pids.parent().unwrap().join("owner");
    fs::create_dir_all(&folder).unwrap();
    let plan = tree_setup(&folder, &pids);
    let _ = create_environment(
        &plan,
        &folder.join("data"),
        EXPECTED,
        &SetupCanceller::new(),
        &mut |_| {},
    );
    std::process::exit(0)
}

#[test]
fn a_real_python_answers_the_check() {
    let Some(found) = real_python() else {
        println!("real Python check SKIPPED: no Python 3.10 or newer found");
        return;
    };
    assert!(found.version.supported());
    assert!(found.executable.is_file(), "{}", found.executable.display());
    let checked = status(Some(&own(&found.executable)), EXPECTED, None);
    // The machine's own Python normally lacks the package; either way it
    // is not too old and is Python.
    assert!(
        matches!(
            &checked,
            PythonStatus::Ready { .. }
                | PythonStatus::Problem {
                    problem: PythonProblem::PackageMissing { .. }
                        | PythonProblem::PackageMismatch { .. },
                    ..
                }
        ),
        "{checked:?}"
    );
}

/// Runs only with Python 3.10+ and Node: a real environment in a temporary
/// data folder, the app's package from the wheel, no libraries (so no
/// network).
#[test]
fn the_app_environment_is_created_from_a_real_python() {
    let Some(base) = real_python() else {
        println!("environment creation SKIPPED: no Python 3.10 or newer found");
        return;
    };
    let folder = TempDir::new().unwrap();
    let Some(wheel) = build_wheel(&folder.path().join("wheel")) else {
        println!("environment creation SKIPPED: Node cannot build the wheel");
        return;
    };
    let version = env!("CARGO_PKG_VERSION");
    let expected = package_version(version);
    let data = folder.path().join("data");
    let offer = SetupOffer::find(
        &[Candidate::new(&base.executable)],
        &environment_folder(&data),
        wheel.parent().unwrap(),
        version,
    )
    .unwrap();
    let plan = offer.plan(&[]);
    let mut steps = Vec::new();
    let made = create_environment(&plan, &data, &expected, &SetupCanceller::new(), &mut |p| {
        if let SetupProgress::Step { description } = p {
            steps.push(description);
        }
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(made.ready(), "{made:?}");
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert!(steps[1].starts_with("Installing the app's package"));

    let choice = PythonChoice::load(&data).unwrap();
    assert_eq!(choice.setup, Setup::Managed);
    assert!(choice.interpreter.starts_with(data.join("python")));
    let PythonStatus::Ready { package, .. } = status(Some(&choice), &expected, None) else {
        panic!("not ready");
    };
    assert_eq!(package, expected);

    // A second setup updates the environment in place.
    let again = SetupOffer::find(
        &[Candidate::new(&base.executable)],
        &environment_folder(&data),
        wheel.parent().unwrap(),
        version,
    )
    .unwrap();
    assert!(again.existing);
}
