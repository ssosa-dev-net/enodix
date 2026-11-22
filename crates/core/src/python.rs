//! The Python environment (#51): which interpreter the Python features use.
//!
//! Python is the user's own and stays optional: until an interpreter is
//! chosen, every Python feature is off ([`PythonStatus::NotSet`], whose
//! message says how to turn them on). There are two ways to choose one, a
//! per-user preference kept in the data folder ([`PythonChoice`]):
//!
//! - **The app's own environment** ([`Setup::Managed`]): [`SetupOffer::find`]
//!   looks for a Python 3.10 or newer on the system ([`system_candidates`]),
//!   the user ticks the libraries wanted ([`Library`]), and
//!   [`SetupOffer::plan`] lists everything that will be installed
//!   ([`SetupPlan::installs`]): the app's own package from the wheel shipped
//!   beside the app, then the libraries from the package index. Nothing is
//!   installed before [`create_environment`] runs that plan, after the user
//!   confirmed it. The environment is a virtual environment in
//!   `<data folder>/python`; the system's Python is never changed.
//! - **The user's own interpreter** ([`Setup::Own`]): only its path is
//!   stored; the app installs nothing into it.
//!
//! Either way [`status`] checks the interpreter each time it is asked: that
//! it runs, is Python 3.10 or newer and can import the app's package in the
//! app's version, each failure with its own message ([`PythonProblem`]).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::process;

/// The oldest Python the package supports.
pub const MIN_VERSION: PythonVersion = PythonVersion {
    major: 3,
    minor: 10,
    patch: 0,
};

/// The app's Python package: its distribution and import name.
pub const PACKAGE: &str = "netterm";

/// The environment's folder inside the data folder.
pub const ENVIRONMENT_FOLDER: &str = "python";

/// The preference's file inside the data folder.
const CHOICE_FILE: &str = "python.yaml";

/// How long an interpreter may take to answer the check.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// What the check runs inside the interpreter. Passed hex-encoded on one
/// line (`exec(bytes.fromhex(...))`), so that no shell or batch-file quoting
/// can damage it. It leaves the current folder out of `sys.path`, so that a
/// stray `netterm` folder there is not taken for the package.
const PROBE: &str = r#"import json, sys
sys.path[:] = [p for p in sys.path if p not in ("", ".")]
info = {"probe": 1, "version": list(sys.version_info[:3]), "executable": sys.executable}
try:
    import netterm
    info["package"] = str(getattr(netterm, "__version__", ""))
except Exception as error:
    info["package"] = None
    info["package_error"] = "%s: %s" % (type(error).__name__, error)
sys.stdout.write(json.dumps(info) + "\n")
"#;

/// A Python version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PythonVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl PythonVersion {
    /// At least [`MIN_VERSION`].
    pub fn supported(self) -> bool {
        self >= MIN_VERSION
    }
}

impl fmt::Display for PythonVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// How the chosen interpreter came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Setup {
    /// The app's own virtual environment in the data folder.
    Managed,
    /// The user's own interpreter; the app installs nothing into it.
    Own,
}

/// The per-user preference: the interpreter the Python features use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonChoice {
    pub interpreter: PathBuf,
    pub setup: Setup,
}

impl PythonChoice {
    /// The choice kept in `data_folder`, or `None` when there is none. An
    /// unreadable or damaged file counts as none (the features stay off and
    /// say how to turn them on), never as an error.
    pub fn load(data_folder: &Path) -> Option<Self> {
        let text = fs::read_to_string(data_folder.join(CHOICE_FILE)).ok()?;
        let choice: Self = serde_norway::from_str(&text).ok()?;
        (!choice.interpreter.as_os_str().is_empty()).then_some(choice)
    }

    /// Keeps the choice in `data_folder` (created if needed), replacing the
    /// file whole (keys `interpreter`, `setup`).
    pub fn save(&self, data_folder: &Path) -> io::Result<()> {
        let text = serde_norway::to_string(self).map_err(io::Error::other)?;
        fs::create_dir_all(data_folder)?;
        let temporary = data_folder.join(format!("{CHOICE_FILE}.tmp"));
        fs::write(&temporary, text)?;
        fs::rename(&temporary, data_folder.join(CHOICE_FILE))
    }

    /// Forgets the choice (the Python features are off again). The app's
    /// environment, if any, stays on disk for a later setup.
    pub fn clear(data_folder: &Path) -> io::Result<()> {
        match fs::remove_file(data_folder.join(CHOICE_FILE)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// The package version the app ships, from the app's (semver) version, as
/// Python writes it (PEP 440): `1.2.3`, `1.2.3-beta.1` → `1.2.3b1`,
/// `-alpha.N` → `aN`, `-rc.N` → `rcN`; build metadata is dropped. The same
/// rule as `scripts/build-wheel.mjs`.
pub fn package_version(app_version: &str) -> String {
    let version = app_version.split('+').next().unwrap_or(app_version);
    let Some((release, stage)) = version.split_once('-') else {
        return version.to_string();
    };
    let short = |name: &str| match name {
        "alpha" => Some("a"),
        "beta" => Some("b"),
        "rc" => Some("rc"),
        _ => None,
    };
    match stage.split_once('.') {
        Some((name, number)) if short(name).is_some() => {
            format!("{release}{}{number}", short(name).unwrap_or_default())
        }
        _ => version.to_string(),
    }
}

/// The wheel's file name for the app version `app_version`.
pub fn wheel_name(app_version: &str) -> String {
    format!(
        "{PACKAGE}-{}-py3-none-any.whl",
        package_version(app_version)
    )
}

/// Where the app's environment is, for the data folder `data_folder`.
pub fn environment_folder(data_folder: &Path) -> PathBuf {
    data_folder.join(ENVIRONMENT_FOLDER)
}

/// A virtual environment's interpreter.
pub fn environment_interpreter(environment: &Path) -> PathBuf {
    if cfg!(windows) {
        environment.join("Scripts").join("python.exe")
    } else {
        environment.join("bin").join("python")
    }
}

/// A way to start a Python: a program and the arguments before ours (the
/// Windows launcher needs `-3`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub program: OsString,
    pub args: Vec<OsString>,
}

impl Candidate {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub fn with_args(program: impl Into<OsString>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
        }
    }
}

impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program.to_string_lossy())?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

/// Where the setup looks for a Python, in order: on Windows the Python
/// launcher (`py -3`), then `python3` and `python` on `PATH`.
pub fn system_candidates() -> Vec<Candidate> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        candidates.push(Candidate::with_args("py", &["-3"]));
    }
    candidates.push(Candidate::new("python3"));
    candidates.push(Candidate::new("python"));
    candidates
}

/// An interpreter that answered the check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interpreter {
    /// The interpreter's own program (`sys.executable`), which is what gets
    /// stored and run, even when it was found through the launcher.
    pub executable: PathBuf,
    pub version: PythonVersion,
    /// The app's package's version, if the interpreter can import it.
    pub package: Option<String>,
    /// Why the package could not be imported, as Python said it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_error: Option<String>,
}

/// Why a program could not be checked as a Python.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProbeError {
    #[error("there is no program at {0}")]
    NotFound(String),
    #[error("{program} could not be started: {reason}")]
    CannotStart { program: String, reason: String },
    #[error("{program} did not answer as Python 3 does{}", detail(.output))]
    NotPython { program: String, output: String },
    #[error("{program} did not answer within {} seconds", PROBE_TIMEOUT.as_secs())]
    TimedOut { program: String },
}

fn detail(output: &str) -> String {
    if output.is_empty() {
        String::new()
    } else {
        format!(": {output}")
    }
}

#[derive(Deserialize)]
struct ProbeAnswer {
    probe: u32,
    version: Vec<u32>,
    executable: PathBuf,
    package: Option<String>,
    #[serde(default)]
    package_error: Option<String>,
}

/// Runs the check inside `candidate`: its version, its own program and
/// whether it imports the app's package.
pub fn probe(candidate: &Candidate) -> Result<Interpreter, ProbeError> {
    let name = candidate.to_string();
    let program = Path::new(&candidate.program);
    // A path (not a bare name looked up on PATH) must exist.
    if program.components().count() > 1 && !program.exists() {
        return Err(ProbeError::NotFound(program.display().to_string()));
    }
    let code = format!("exec(bytes.fromhex('{}').decode())", hex(PROBE));
    let mut command = quiet_command(&candidate.program);
    command.args(&candidate.args).arg("-c").arg(code);
    let mut lines = Vec::new();
    let ended = run(command, Some(PROBE_TIMEOUT), None, &mut |line| {
        lines.push(line)
    })
    .map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ProbeError::NotFound(name.clone()),
        _ => ProbeError::CannotStart {
            program: name.clone(),
            reason: error.to_string(),
        },
    })?;
    let output = || last_lines(&lines, 3);
    match ended {
        Ended::TimedOut | Ended::Cancelled => {
            return Err(ProbeError::TimedOut { program: name });
        }
        Ended::Exited(status) if !status.success() => {
            return Err(ProbeError::NotPython {
                program: name,
                output: output(),
            });
        }
        Ended::Exited(_) => {}
    }
    let answer = lines
        .iter()
        .rev()
        .filter(|line| line.trim_start().starts_with('{'))
        .find_map(|line| serde_json::from_str::<ProbeAnswer>(line.trim()).ok())
        .filter(|answer| answer.probe == 1 && answer.version.len() >= 2);
    let Some(answer) = answer else {
        return Err(ProbeError::NotPython {
            program: name,
            output: output(),
        });
    };
    Ok(Interpreter {
        executable: answer.executable,
        version: PythonVersion {
            major: answer.version[0],
            minor: answer.version[1],
            patch: answer.version.get(2).copied().unwrap_or(0),
        },
        package: answer.package,
        package_error: answer.package_error,
    })
}

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn last_lines(lines: &[String], count: usize) -> String {
    let kept: Vec<&str> = lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    kept[kept.len().saturating_sub(count)..].join(" | ")
}

/// Why no Python could be offered for the app's environment.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FindError {
    #[error(
        "No Python was found (looked for {looked}). Install Python {MIN_VERSION_SHORT} or newer \
         (python.org), then try again, or use an interpreter of your own."
    )]
    NoPython { looked: String },
    #[error(
        "Only Python {found} was found; {MIN_VERSION_SHORT} or newer is needed. Install a newer \
         Python (python.org), then try again, or use an interpreter of your own."
    )]
    TooOld { found: String },
}

const MIN_VERSION_SHORT: &str = "3.10";

/// The first candidate that is a Python 3.10 or newer.
pub fn find_python(candidates: &[Candidate]) -> Result<Interpreter, FindError> {
    let mut too_old: Vec<String> = Vec::new();
    for candidate in candidates {
        let Ok(interpreter) = probe(candidate) else {
            continue;
        };
        if interpreter.version.supported() {
            return Ok(interpreter);
        }
        let found = format!("{} ({candidate})", interpreter.version);
        if !too_old.contains(&found) {
            too_old.push(found);
        }
    }
    if too_old.is_empty() {
        let looked: Vec<String> = candidates.iter().map(ToString::to_string).collect();
        Err(FindError::NoPython {
            looked: looked.join(", "),
        })
    } else {
        Err(FindError::TooOld {
            found: too_old.join(", "),
        })
    }
}

/// What is wrong with the chosen interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PythonProblem {
    /// Nothing runs at the stored path.
    Missing,
    /// Something runs there, but it is not a Python 3 (or it hung).
    NotPython { reason: String },
    /// A Python older than 3.10.
    TooOld { version: PythonVersion },
    /// The interpreter cannot import the app's package.
    PackageMissing {
        version: PythonVersion,
        reason: Option<String>,
    },
    /// The package is there but in another version than the app's.
    PackageMismatch {
        version: PythonVersion,
        installed: String,
        expected: String,
    },
}

/// What the Python features can count on, with a message to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PythonStatus {
    /// No interpreter chosen: every Python feature is off.
    NotSet { message: String },
    /// The interpreter works and has the app's package.
    Ready {
        interpreter: PathBuf,
        setup: Setup,
        version: PythonVersion,
        package: String,
        message: String,
    },
    /// An interpreter is chosen but cannot be used as it is.
    Problem {
        interpreter: PathBuf,
        setup: Setup,
        problem: PythonProblem,
        message: String,
    },
}

/// What the Python features say while no interpreter is chosen.
pub const NOT_SET_MESSAGE: &str = "Python features are off. To turn them on, open Preferences → \
     Python and either let the app create its own Python environment or choose an interpreter \
     of your own.";

impl PythonStatus {
    /// The Python features can run.
    pub fn ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn message(&self) -> &str {
        match self {
            Self::NotSet { message } | Self::Ready { message, .. } => message,
            Self::Problem { message, .. } => message,
        }
    }
}

/// Checks the chosen interpreter. `expected_package` is the package version
/// the app ships ([`package_version`]); `wheel` is where the shipped wheel
/// is, for the advice on installing it (`None` when it is not found).
pub fn status(
    choice: Option<&PythonChoice>,
    expected_package: &str,
    wheel: Option<&Path>,
) -> PythonStatus {
    let Some(choice) = choice else {
        return PythonStatus::NotSet {
            message: NOT_SET_MESSAGE.into(),
        };
    };
    let interpreter = choice.interpreter.clone();
    let setup = choice.setup;
    let problem = |problem: PythonProblem| {
        let message = problem_message(&problem, choice, expected_package, wheel);
        PythonStatus::Problem {
            interpreter: interpreter.clone(),
            setup,
            problem,
            message,
        }
    };
    let found = match probe(&Candidate::new(&choice.interpreter)) {
        Ok(found) => found,
        Err(ProbeError::NotFound(_)) => return problem(PythonProblem::Missing),
        Err(error) => {
            return problem(PythonProblem::NotPython {
                reason: error.to_string(),
            })
        }
    };
    let version = found.version;
    if !version.supported() {
        return problem(PythonProblem::TooOld { version });
    }
    match found.package {
        None => problem(PythonProblem::PackageMissing {
            version,
            reason: found.package_error,
        }),
        Some(installed) if installed != expected_package => {
            problem(PythonProblem::PackageMismatch {
                version,
                installed,
                expected: expected_package.into(),
            })
        }
        Some(package) => PythonStatus::Ready {
            message: format!(
                "Python {version} with the app's package {package}: Python features are on."
            ),
            interpreter,
            setup,
            version,
            package,
        },
    }
}

fn problem_message(
    problem: &PythonProblem,
    choice: &PythonChoice,
    expected: &str,
    wheel: Option<&Path>,
) -> String {
    let path = choice.interpreter.display();
    let managed = choice.setup == Setup::Managed;
    let set_up_again = "Set up the app's environment again, or choose another interpreter.";
    let install = match wheel {
        Some(wheel) => format!(
            "Install it into that interpreter with: \"{path}\" -m pip install \"{}\"",
            wheel.display()
        ),
        None => format!(
            "Install it into that interpreter from the wheel in the app's python folder \
             ({}).",
            wheel_file(expected)
        ),
    };
    match problem {
        PythonProblem::Missing if managed => {
            format!("The app's Python environment is gone ({path} does not exist). {set_up_again}")
        }
        PythonProblem::Missing => format!(
            "No Python at {path}: the file does not exist. Choose the interpreter again \
             (for example a virtual environment's own interpreter)."
        ),
        PythonProblem::NotPython { reason } => {
            format!("{path} cannot be used as Python: {reason}.")
        }
        PythonProblem::TooOld { version } => format!(
            "{path} is Python {version}; Python {MIN_VERSION_SHORT} or newer is needed. Choose a \
             newer interpreter."
        ),
        PythonProblem::PackageMissing { .. } if managed => format!(
            "The app's Python environment lacks the app's package ({PACKAGE}). {set_up_again}"
        ),
        PythonProblem::PackageMissing { .. } => {
            format!("{path} cannot import the app's package ({PACKAGE} {expected}). {install}")
        }
        PythonProblem::PackageMismatch {
            installed,
            expected,
            ..
        } if managed => format!(
            "The app's Python environment has the package {PACKAGE} {installed}, but this app \
             ships {expected}. {set_up_again}"
        ),
        PythonProblem::PackageMismatch {
            installed,
            expected,
            ..
        } => format!(
            "{path} has the package {PACKAGE} {installed}, but this app ships {expected}. \
             {install}"
        ),
    }
}

fn wheel_file(expected: &str) -> String {
    format!("{PACKAGE}-{expected}-py3-none-any.whl")
}

/// A library the user can have installed into the app's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Library {
    Nornir,
    Netmiko,
    Scrapli,
    Napalm,
}

impl Library {
    pub const ALL: [Library; 4] = [
        Library::Nornir,
        Library::Netmiko,
        Library::Scrapli,
        Library::Napalm,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Library::Nornir => "Nornir",
            Library::Netmiko => "Netmiko",
            Library::Scrapli => "scrapli",
            Library::Napalm => "NAPALM",
        }
    }

    /// What it is for, in a few words.
    pub fn purpose(self) -> &'static str {
        match self {
            Library::Nornir => "runs tasks on many hosts; reads the workspace as its inventory",
            Library::Netmiko => "SSH to network devices, command by command",
            Library::Scrapli => "fast SSH and telnet to network devices",
            Library::Napalm => "one API for configurations and facts of many vendors",
        }
    }

    /// Its requirement for pip.
    fn requirement(self) -> &'static str {
        match self {
            // The inventory plugin is written for Nornir 3.
            Library::Nornir => "nornir>=3.3,<4",
            Library::Netmiko => "netmiko",
            Library::Scrapli => "scrapli",
            Library::Napalm => "napalm",
        }
    }

    /// Nornir's plugin for this library, installed when both are ticked.
    fn nornir_plugin(self) -> Option<&'static str> {
        match self {
            Library::Nornir => None,
            Library::Netmiko => Some("nornir-netmiko"),
            Library::Scrapli => Some("nornir-scrapli"),
            Library::Napalm => Some("nornir-napalm"),
        }
    }
}

/// A library as the setup offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferedLibrary {
    pub library: Library,
    pub label: &'static str,
    pub purpose: &'static str,
}

/// Where an install comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InstallSource {
    /// The wheel shipped with the app (never the package index).
    Wheel { path: PathBuf },
    /// The package index (PyPI, or what pip is configured to use), with
    /// whatever the package itself needs.
    Index,
}

/// One thing the setup installs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedInstall {
    /// The package, as pip is given it (`nornir>=3.3,<4`, `netmiko`, or
    /// the wheel's file name).
    pub name: String,
    pub source: InstallSource,
}

/// Why the app's environment cannot be offered.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OfferError {
    #[error(transparent)]
    Find(#[from] FindError),
    #[error(
        "The app's Python package ({file}) is not in {folder}, where the app ships it. \
         Reinstall the app, or use an interpreter of your own."
    )]
    NoWheel { file: String, folder: String },
}

/// The app's environment as it can be set up: from this Python, at this
/// place, with this wheel. Nothing has been installed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupOffer {
    /// The Python found on the system, which creates the environment.
    pub base: Interpreter,
    /// The environment's folder.
    pub environment: PathBuf,
    /// An environment is there already (it is updated in place).
    pub existing: bool,
    /// The app's package.
    pub wheel: PathBuf,
    /// What the user can tick.
    pub libraries: Vec<OfferedLibrary>,
}

impl SetupOffer {
    /// Finds a Python 3.10 or newer among `candidates` and the wheel of
    /// `app_version` in `wheel_folder`, for an environment in the folder
    /// `environment` (the app's: `DataFolders::python_environment`, since
    /// #128 in the local data folder). Installs nothing.
    pub fn find(
        candidates: &[Candidate],
        environment: &Path,
        wheel_folder: &Path,
        app_version: &str,
    ) -> Result<Self, OfferError> {
        // Python first: its absence is the likelier, more useful message.
        let base = find_python(candidates)?;
        let file = wheel_name(app_version);
        let wheel = wheel_folder.join(&file);
        if !wheel.is_file() {
            return Err(OfferError::NoWheel {
                file,
                folder: wheel_folder.display().to_string(),
            });
        }
        let environment = environment.to_path_buf();
        Ok(Self {
            existing: environment_interpreter(&environment).exists(),
            base,
            environment,
            wheel,
            libraries: Library::ALL
                .iter()
                .map(|&library| OfferedLibrary {
                    library,
                    label: library.label(),
                    purpose: library.purpose(),
                })
                .collect(),
        })
    }

    /// The plan for the ticked `libraries` (order and repeats do not
    /// matter): everything it will install, in order.
    pub fn plan(&self, libraries: &[Library]) -> SetupPlan {
        let mut ticked: Vec<Library> = libraries.to_vec();
        ticked.sort();
        ticked.dedup();
        let wheel_name = self
            .wheel
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut installs = vec![PlannedInstall {
            name: wheel_name,
            source: InstallSource::Wheel {
                path: self.wheel.clone(),
            },
        }];
        let index = |name: &str| PlannedInstall {
            name: name.into(),
            source: InstallSource::Index,
        };
        for library in &ticked {
            installs.push(index(library.requirement()));
        }
        if ticked.contains(&Library::Nornir) {
            for library in &ticked {
                if let Some(plugin) = library.nornir_plugin() {
                    installs.push(index(plugin));
                }
            }
        }
        SetupPlan {
            base: self.base.clone(),
            environment: self.environment.clone(),
            interpreter: environment_interpreter(&self.environment),
            existing: self.existing,
            libraries: ticked,
            installs,
        }
    }
}

/// Exactly what the setup will do, shown to the user before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    pub base: Interpreter,
    pub environment: PathBuf,
    /// The environment's interpreter, the one that will be chosen.
    pub interpreter: PathBuf,
    pub existing: bool,
    pub libraries: Vec<Library>,
    /// The app's package first, then the libraries and Nornir's plugins.
    pub installs: Vec<PlannedInstall>,
}

/// What the setup is doing, as it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SetupProgress {
    /// A step begins.
    Step { description: String },
    /// A line of the step's output (pip's, venv's).
    Output { line: String },
}

/// Stops a running setup (the step's process is ended, with what it
/// started), and tells when it has stopped.
#[derive(Debug, Clone, Default)]
pub struct SetupCanceller(Arc<SetupState>);

#[derive(Debug, Default)]
struct SetupState {
    cancelled: AtomicBool,
    /// How many [`create_environment`] calls run with it.
    running: Mutex<usize>,
    ended: Condvar,
}

impl SetupCanceller {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Waits up to `within` until no setup runs with it (closing the app
    /// after [`cancel`](Self::cancel)); whether none does.
    pub fn wait(&self, within: Duration) -> bool {
        let running = self
            .0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (running, _) = self
            .0
            .ended
            .wait_timeout_while(running, within, |count| *count > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *running == 0
    }

    /// Counts a setup as running with it until the guard is dropped.
    fn running(&self) -> SetupRunning<'_> {
        *self
            .0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        SetupRunning(self)
    }
}

struct SetupRunning<'a>(&'a SetupCanceller);

impl Drop for SetupRunning<'_> {
    fn drop(&mut self) {
        let state = &(self.0).0;
        *state.running.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        state.ended.notify_all();
    }
}

/// Why the setup did not finish.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SetupError {
    #[error(
        "Cancelled while {step}. The environment may be incomplete; set it up again to finish it."
    )]
    Cancelled { step: String },
    #[error("Failed while {step}: {reason}")]
    Failed { step: String, reason: String },
    #[error(
        "The environment was set up but does not work: {0} Set it up again, or use an \
         interpreter of your own."
    )]
    NotWorking(String),
    #[error("The environment works, but the choice could not be saved: {0}")]
    NotSaved(String),
}

/// Creates (or updates) the app's environment as `plan` says, installs the
/// app's package from the wheel (never the index) and the libraries from the
/// index, checks the result and stores it as the choice in `data_folder`.
/// `progress` hears each step and every output line.
pub fn create_environment(
    plan: &SetupPlan,
    data_folder: &Path,
    expected_package: &str,
    canceller: &SetupCanceller,
    progress: &mut dyn FnMut(SetupProgress),
) -> Result<PythonStatus, SetupError> {
    let _running = canceller.running();
    let interpreter = plan.interpreter.as_os_str();
    let pip = |args: &[&OsStr]| {
        let mut all: Vec<OsString> = ["-m", "pip", "install", "--disable-pip-version-check"]
            .iter()
            .map(OsString::from)
            .collect();
        all.extend(args.iter().map(|arg| arg.to_os_string()));
        all
    };
    let mut steps: Vec<(String, &OsStr, Vec<OsString>)> = vec![(
        format!(
            "{} the environment in {} with Python {}",
            if plan.existing {
                "updating"
            } else {
                "creating"
            },
            plan.environment.display(),
            plan.base.version
        ),
        plan.base.executable.as_os_str(),
        vec!["-m".into(), "venv".into(), plan.environment.clone().into()],
    )];
    for install in &plan.installs {
        match &install.source {
            InstallSource::Wheel { path } => steps.push((
                format!("installing the app's package from {}", path.display()),
                interpreter,
                pip(&[
                    OsStr::new("--no-index"),
                    OsStr::new("--force-reinstall"),
                    OsStr::new("--no-deps"),
                    path.as_os_str(),
                ]),
            )),
            InstallSource::Index => {}
        }
    }
    let from_index: Vec<OsString> = plan
        .installs
        .iter()
        .filter(|install| install.source == InstallSource::Index)
        .map(|install| OsString::from(&install.name))
        .collect();
    if !from_index.is_empty() {
        let names: Vec<String> = from_index
            .iter()
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        let args: Vec<&OsStr> = from_index.iter().map(OsString::as_os_str).collect();
        steps.push((
            format!("installing {} from the package index", names.join(", ")),
            interpreter,
            pip(&args),
        ));
    }
    for (step, program, args) in steps {
        progress(SetupProgress::Step {
            description: capitalised(&step),
        });
        let mut command = quiet_command(program);
        command.args(&args).env("PIP_NO_INPUT", "1");
        let mut lines = Vec::new();
        let ended = run(command, None, Some(canceller), &mut |line| {
            lines.push(line.clone());
            progress(SetupProgress::Output { line });
        })
        .map_err(|error| SetupError::Failed {
            step: step.clone(),
            reason: format!(
                "{} could not be started: {error}",
                Path::new(program).display()
            ),
        })?;
        match ended {
            Ended::Exited(status) if status.success() => {}
            Ended::Exited(status) => {
                let output = last_lines(&lines, 3);
                return Err(SetupError::Failed {
                    step,
                    reason: format!("it ended with {}{}", exit_code(status), detail(&output)),
                });
            }
            Ended::Cancelled | Ended::TimedOut => return Err(SetupError::Cancelled { step }),
        }
    }
    let choice = PythonChoice {
        interpreter: plan.interpreter.clone(),
        setup: Setup::Managed,
    };
    progress(SetupProgress::Step {
        description: "Checking the environment".into(),
    });
    let checked = status(Some(&choice), expected_package, None);
    if !checked.ready() {
        return Err(SetupError::NotWorking(checked.message().to_string()));
    }
    choice
        .save(data_folder)
        .map_err(|error| SetupError::NotSaved(error.to_string()))?;
    Ok(checked)
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn exit_code(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => "no exit status (it was ended)".into(),
    }
}

/// The variable a bundle's runtime sets to the folder the bundle is
/// mounted at (an AppImage's `APPDIR`).
pub const BUNDLE_VARIABLE: &str = "APPDIR";

/// Leaves out of `command`'s environment what a bundle's runtime added
/// for the app and that points into the bundle ([`BUNDLE_VARIABLE`]): an
/// AppImage sets `PYTHONHOME`, `PYTHONPATH`, `LD_LIBRARY_PATH` and others
/// there, with which the system's own Python cannot even start ("No module
/// named 'encodings'"). A variable holding a list keeps its other entries.
/// The desktop's openers get the same ([`crate::opener`]).
pub(crate) fn leave_the_bundle(command: &mut Command) {
    let Some(bundle) = std::env::var_os(BUNDLE_VARIABLE).filter(|folder| !folder.is_empty()) else {
        return;
    };
    let bundle = PathBuf::from(bundle);
    let inside = |path: &PathBuf| path.starts_with(&bundle);
    for (name, value) in std::env::vars_os() {
        if name == BUNDLE_VARIABLE {
            continue;
        }
        let parts: Vec<PathBuf> = std::env::split_paths(&value).collect();
        if !parts.iter().any(inside) {
            continue;
        }
        let kept: Vec<PathBuf> = parts
            .into_iter()
            .filter(|part| !part.as_os_str().is_empty() && !inside(part))
            .collect();
        match std::env::join_paths(&kept) {
            Ok(joined) if !kept.is_empty() => command.env(&name, joined),
            _ => command.env_remove(&name),
        };
    }
}

/// A command with no window of its own (on Windows a console would flash
/// up for every check), its input closed and its output captured.
///
/// Elsewhere it starts in a session of its own, so that it has no
/// terminal either (as a Windows process without a console: the headless
/// binary then never waits on a password prompt nobody sees) and its
/// process group holds the processes it starts (`crate::process`). Start
/// it with `crate::process::start`. Started from an AppImage, it gets none
/// of the bundle's variables ([`leave_the_bundle`]).
pub(crate) fn quiet_command(program: &OsStr) -> Command {
    let mut command = Command::new(program);
    leave_the_bundle(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(std::env::temp_dir())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::process::NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and touches no memory of
        // the parent.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    command
}

enum Ended {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

/// Runs `command`, handing each output line (stdout and stderr, as they
/// come) to `on_line`, until it ends, `timeout` passes or `canceller` is
/// cancelled (then the process is ended).
fn run(
    mut command: Command,
    timeout: Option<Duration>,
    canceller: Option<&SetupCanceller>,
    on_line: &mut dyn FnMut(String),
) -> io::Result<Ended> {
    // The program and what it starts end with it, or with this process
    // (`crate::process`).
    let mut tree = process::start(&mut command)?;
    let (sender, lines) = mpsc::channel::<String>();
    let readers = [
        tree.child
            .stdout
            .take()
            .map(|out| read_lines(out, sender.clone())),
        tree.child
            .stderr
            .take()
            .map(|err| read_lines(err, sender.clone())),
    ];
    drop(sender);
    let started = Instant::now();
    let ended = loop {
        match lines.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => on_line(line),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Both pipes closed (the process may still run): keep polling,
            // so that the timeout and cancelling still apply.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
        if canceller.is_some_and(SetupCanceller::cancelled) {
            tree.stop();
            break Ended::Cancelled;
        }
        if timeout.is_some_and(|limit| started.elapsed() > limit) {
            tree.stop();
            break Ended::TimedOut;
        }
        if let Some(status) = tree.child.try_wait()? {
            // Whatever is still in the pipes, for a moment (a process it
            // left behind may hold them open).
            let until = Instant::now() + Duration::from_millis(500);
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                match lines.recv_timeout(left) {
                    Ok(line) => on_line(line),
                    Err(_) => break,
                }
            }
            break Ended::Exited(status);
        }
    };
    if matches!(ended, Ended::Exited(_)) {
        for reader in readers.into_iter().flatten() {
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
    }
    Ok(ended)
}

fn read_lines(
    stream: impl Read + Send + 'static,
    sender: mpsc::Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim_end_matches(['\r', '\n']).to_string();
                    if sender.send(text).is_err() {
                        break;
                    }
                }
            }
        }
    })
}
