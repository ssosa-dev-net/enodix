//! Jobs: a saved snippet run on many hosts at once, unattended.
//!
//! A job runs in two parts. [`plan`] takes the workspace, the credential
//! profiles, a [`JobTarget`] (a folder, or a list of hosts such as a saved
//! filter's) and a [`JobStep`], and works out the pre-flight list without
//! connecting to anything: the hosts that will run, the hosts that are
//! skipped and why ([`SkipReason`]), the production hosts among those that
//! run, and the snippet variables no host fills (asked once, for all).
//! [`start`] then runs a ready plan: each host gets its own headless
//! session, opened through the same path as a terminal session to that
//! host (its profile target with its jump hosts, the manager's known-hosts
//! store, legacy crypto, agents, connect timeout). Once the prompt shows,
//! the job enters privileged mode if the platform definition has a
//! privilege command (answering its password question with the profile's
//! enable secret), turns paging off with the definition's disable-paging
//! command, and sends the snippet line by line, waiting for the prompt
//! after each. A host that shows no prompt within the command timeout
//! fails; at most [`JobOptions::parallel`] hosts run at once. Progress
//! arrives as [`JobEvent`]s on the returned [`Job`], which can cancel the
//! run.
//!
//! A job never asks anything: an untrusted host key is refused and a login
//! question is not answered, and either fails that host with a reason that
//! names it. One host's failure never stops the others. Job sessions are
//! never logged and never touch open terminals (each is its own session).
//!
//! The [`JobStep::ConfigBackup`] step sends the definition's show-config
//! command instead of a snippet and keeps what it showed (the command's
//! echo and the final prompt left out) in a [`crate::configs::ConfigStore`]
//! as a new version of the host's configuration, if it changed.
//!
//! The [`JobStep::Script`] step runs a Python script of the workspace
//! instead (see [`script`]): one process for the whole job, which reaches
//! the hosts itself through the inventory plugin, limited to the job's
//! hosts. Pre-flight and the production confirmation are the same.
//!
//! A job can be saved under a name in the workspace ([`SavedJob`], see
//! [`crate::workspace::JOBS_FILE`]) and planned again in one action with
//! [`plan_saved`].
//!
//! Every run is kept, outside the workspace, by [`results::RunStore`]:
//! listed newest first, searched, compared line by line, exported, deleted,
//! and planned again on the same target with [`plan_again`].

mod live;
mod login;
pub mod results;
mod run;
pub mod script;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::credentials::{HostLogin, ProfileError, Profiles};
use crate::session::{SshAuth, SshTarget};
use crate::template::Variables;
use crate::workspace::{
    Environment, ExpandError, Folder, HostId, Protocol, ResolvedHost, SavedJob, ScriptError,
    Workspace,
};

pub use live::{LiveJob, LiveJobs, NewLiveJob};
pub use login::{LoginBackup, LoginBackupOutcome, SessionLogin};
pub use run::{
    check, start, HostResult, HostState, Job, JobCanceller, JobEvent, JobOptions, JobReport,
    StartError, DEFAULT_COMMAND_TIMEOUT, DEFAULT_PARALLEL, JOB_TERMINAL_SIZE, MAX_PARALLEL,
    PROMPT_SETTLE,
};
pub use script::{ScriptOptions, ScriptTargets, SCRIPT_OUTPUT};

/// Which hosts a job runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JobTarget {
    /// Every host in the folder at `path` (`""` is the root) and in the
    /// folders below it.
    Folder { path: String },
    /// The hosts the saved filter `name` shows when the job is planned, in
    /// tree order.
    Filter { name: String },
    /// These hosts, in this order (a single host, say).
    Hosts { ids: Vec<HostId> },
}

/// What a job does on each host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JobStep {
    /// Sends the saved snippet `name`, line by line, its variables filled
    /// from each host's values and then from `answers` (the same for every
    /// host; an empty answer counts).
    Snippet {
        name: String,
        #[serde(default)]
        answers: BTreeMap<String, String>,
    },
    /// Fetches the configuration with the platform definition's
    /// show-config command and keeps it in the config history if it
    /// changed (see [`crate::configs`]).
    ConfigBackup,
    /// Runs the Python script at `path` (relative to the workspace root,
    /// `/`-separated) once, for all the hosts: it reaches them itself,
    /// through the inventory plugin, which shows it only the job's hosts.
    Script { path: String },
    /// Fetches each host's vendor, model, serial number and OS version
    /// with NAPALM, through the script the app ships (see
    /// [`crate::enrich`]); the facts are kept with the run, to preview and
    /// apply.
    Enrich,
}

impl JobStep {
    /// Whether the step runs a Python script (once, for all its hosts).
    pub fn runs_script(&self) -> bool {
        matches!(self, JobStep::Script { .. } | JobStep::Enrich)
    }
}

/// The pre-flight list of a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobPlan {
    pub step: JobStep,
    /// The hosts that will run, in target order.
    pub hosts: Vec<PlannedHost>,
    /// The hosts that will not run, with why, in target order.
    pub skipped: Vec<SkippedHost>,
    /// The hosts that will run and are marked production: the user
    /// confirms them once before the job starts.
    pub production: Vec<HostId>,
    /// The snippet variables some host that will run does not fill and
    /// the answers do not either, in the snippet's order: ask for them once
    /// and plan again with the answers. The plan cannot start until this is
    /// empty.
    pub missing_variables: Vec<String>,
    /// The id of the workspace it was planned in, where a config backup
    /// keeps what it fetched.
    #[serde(skip)]
    workspace: Option<String>,
    /// For a script step: the script's file and the workspace folder.
    #[serde(skip)]
    script: Option<PlannedScript>,
}

/// The script a script step runs, as planned.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedScript {
    /// The workspace folder: the script's working folder.
    root: PathBuf,
    /// What the interpreter runs.
    run: ScriptRun,
}

/// What the interpreter of a script step runs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ScriptRun {
    /// A script file of the workspace.
    File(PathBuf),
    /// A module of the installed packages (`-m`), such as the app's own.
    Module(&'static str),
}

impl JobPlan {
    /// Whether [`start`] can run it: something runs and no variable is
    /// missing.
    pub fn ready(&self) -> bool {
        !self.hosts.is_empty() && self.missing_variables.is_empty()
    }
}

/// A host that will run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedHost {
    pub id: HostId,
    /// The host's name, for the user.
    pub name: String,
    /// The management address it is reached at (perhaps through jump
    /// hosts).
    pub address: String,
    /// The environment it is marked with; production hosts are confirmed
    /// before the job starts.
    pub environment: Option<Environment>,
    /// The name of the platform definition that drives it.
    pub platform: String,
    /// What the job sends and how it connects: not shown.
    #[serde(skip)]
    work: HostWork,
}

/// How a planned host is reached and driven.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostWork {
    /// The profile target, jump hosts included; for a backup on login,
    /// the second connection a device that refuses a second channel gets
    /// (`None`: it gets none).
    target: Option<SshTarget>,
    /// The name of the credential profile it logs in with (`None`: none
    /// applies, for a backup on login with a typed login).
    profile: Option<String>,
    /// The profile's enable secret, if it has one.
    enable_secret: Option<EnableSecret>,
    /// The definition's prompt pattern.
    prompt: String,
    /// The definition's privilege command, if it has one.
    privilege_command: Option<String>,
    /// The definition's pattern of the privilege password question.
    privilege_prompt: Option<String>,
    /// The definition's disable-paging command, if it has one.
    disable_paging: Option<String>,
    /// The snippet's lines for this host; empty while variables are
    /// missing (and for a config backup).
    lines: Vec<String>,
    /// For a config backup: what fetches the configuration.
    backup: Option<BackupWork>,
    /// For filling in from devices: the NAPALM driver.
    napalm_driver: Option<&'static str>,
}

/// How a config backup fetches a host's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BackupWork {
    /// The definition's show-config command.
    command: String,
    /// The definition's ignored-lines pattern, if it has one.
    ignored_lines: Option<String>,
}

/// An enable secret, kept out of `Debug` and wiped when dropped.
#[derive(Clone, PartialEq, Eq)]
struct EnableSecret(String);

impl std::fmt::Debug for EnableSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnableSecret(<hidden>)")
    }
}

impl Drop for EnableSecret {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

/// `value`, unless it is missing or blank.
fn given(value: &Option<String>) -> Option<String> {
    value.clone().filter(|value| !value.trim().is_empty())
}

/// A host that will not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedHost {
    pub id: HostId,
    /// The host's name, for the user (its id if it is gone).
    pub name: String,
    pub reason: SkipReason,
    /// The reason, for the user.
    pub message: String,
}

/// Why a host does not run in a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SkipReason {
    /// The host is no longer in the workspace (a list made before a
    /// reload, say).
    #[error("it is no longer in the workspace")]
    Gone,
    /// Jobs only run over SSH.
    #[error("it is reached over {}, and jobs only run over SSH", .protocol.as_str())]
    NotSsh { protocol: Protocol },
    /// No platform applies to it, or the one that applies names no usable
    /// definition.
    #[error("{}", no_platform(.platform))]
    NoPlatform { platform: Option<String> },
    /// A config backup, and its platform definition has no show-config
    /// command.
    #[error("its platform {platform} has no show-config command, so a backup cannot fetch its configuration")]
    NoShowConfig { platform: String },
    /// No credential profile applies to it: a job cannot ask for a login.
    #[error("it has no credential profile, and a job cannot ask for a login")]
    NoProfile,
    /// Its profile is not stored on this computer.
    #[error("its credential profile {profile} is not set up on this computer")]
    ProfileNotSetUp { profile: String },
    /// Filling in from devices, and its platform definition has no NAPALM
    /// driver.
    #[error("its platform {platform} has no NAPALM driver, so its facts cannot be fetched")]
    NoNapalmDriver { platform: String },
    /// A script step (filling in from devices too), and it is reached
    /// through a jump host that has no usable profile, so the script's
    /// tunnel (see [`crate::tunnel`]) cannot log in to it.
    #[error("it is reached through the jump host {jump_host}, which has no stored credential profile, so the script's tunnel cannot log in to it")]
    BehindJumpHost { jump_host: String },
    /// A script step, and it is reached over telnet through a jump host:
    /// scripts are tunnelled through jump hosts over SSH only.
    #[error("it is reached over telnet through the jump host {jump_host}, and scripts reach hosts behind jump hosts only over SSH")]
    TelnetBehindJumpHost { jump_host: String },
    /// Its jump hosts cannot be worked out (missing, a loop, not SSH).
    #[error("its jump hosts cannot be used: {error}")]
    JumpChain { error: String },
    /// A jump host on the way has no usable profile, so it would ask for a
    /// typed login.
    #[error("its jump host {jump_host} has no stored credential profile, and a job cannot ask for its login")]
    JumpHostNeedsLogin { jump_host: String },
    /// The credential store could not be read (a locked vault, say).
    #[error("{error}")]
    Credentials { error: String },
}

fn no_platform(platform: &Option<String>) -> String {
    match platform {
        None => "it has no platform, so the job does not know its prompt".into(),
        Some(platform) => format!(
            "its platform {platform} has no usable platform definition, so the job does not know its prompt"
        ),
    }
}

/// Why a job cannot be planned at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// A folder target that the workspace does not have.
    #[error("there is no folder {0:?} in the workspace")]
    NoSuchFolder(String),
    /// A filter target that the workspace does not have.
    #[error("there is no saved filter called {0}")]
    NoSuchFilter(String),
    /// A saved job whose one host the workspace no longer has.
    #[error("{0} is no longer in the workspace")]
    NoSuchHost(HostId),
    /// A saved job that the workspace does not have.
    #[error("there is no saved job called {0}")]
    NoSuchJob(String),
    /// A snippet step that the workspace does not have.
    #[error("there is no snippet called {0}")]
    NoSuchSnippet(String),
    /// A script step whose script the workspace does not have.
    #[error("{0}")]
    NoSuchScript(#[from] ScriptError),
}

/// Works out the pre-flight list of running `step` on `target`, connecting
/// to nothing. A host runs if it is reached over SSH, a usable platform
/// definition applies to it, its credential profile is stored, and its jump
/// hosts (if any) all have stored profiles too; every other host is
/// skipped with the first of those reasons that applies.
///
/// A script step runs on every host of the target that the inventory
/// plugin can hand over and reach: a host whose credential profile is
/// named but not stored here (the plugin would refuse to load) is skipped,
/// and so is a host behind jump hosts that cannot be tunnelled (a broken
/// chain, a jump host with no stored profile, telnet). How the script
/// reaches the others (protocol, platform) is its own business, as for a
/// script run from a shell; hosts behind jump hosts it reaches through the
/// plugin's tunnels.
pub fn plan(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    target: &JobTarget,
    step: &JobStep,
) -> Result<JobPlan, PlanError> {
    if let JobStep::Snippet { name, .. } = step {
        if workspace.snippet(name).is_none() {
            return Err(PlanError::NoSuchSnippet(name.clone()));
        }
    }

    let script = match step {
        JobStep::Script { path } => Some(PlannedScript {
            root: workspace.root().to_path_buf(),
            run: ScriptRun::File(workspace.script_file(path)?),
        }),
        JobStep::Enrich => Some(PlannedScript {
            root: workspace.root().to_path_buf(),
            run: ScriptRun::Module(crate::enrich::ENRICH_MODULE),
        }),
        _ => None,
    };

    let mut plan = JobPlan {
        step: step.clone(),
        hosts: Vec::new(),
        skipped: Vec::new(),
        production: Vec::new(),
        missing_variables: Vec::new(),
        workspace: workspace.id().map(str::to_string),
        script,
    };
    for id in target_hosts(workspace, target)? {
        let Some(host) = workspace.resolve_host(&id) else {
            plan.skipped.push(SkippedHost {
                name: id.to_string(),
                id,
                message: SkipReason::Gone.to_string(),
                reason: SkipReason::Gone,
            });
            continue;
        };
        let runs = match step {
            JobStep::Script { .. } => script_runnable(workspace, profiles, &host),
            JobStep::Enrich => enrich_runnable(workspace, profiles, &host),
            _ => runnable(workspace, profiles, &host, step),
        };
        match runs {
            Ok((work, platform)) => {
                let environment = host.settings.environment.as_ref().map(|e| e.value);
                if environment == Some(Environment::Production) {
                    plan.production.push(id.clone());
                }
                plan.hosts.push(PlannedHost {
                    id,
                    name: host.host.name.clone(),
                    address: host.host.address.clone(),
                    environment,
                    platform,
                    work,
                });
            }
            Err(reason) => plan.skipped.push(SkippedHost {
                id,
                name: host.host.name.clone(),
                message: reason.to_string(),
                reason,
            }),
        }
    }

    let JobStep::Snippet { name, answers } = step else {
        return Ok(plan);
    };
    let answers: Variables = answers.iter().collect();
    let ids: Vec<Option<HostId>> = plan.hosts.iter().map(|h| Some(h.id.clone())).collect();
    match workspace.expand_snippet_for_each(name, &ids, &answers) {
        Ok(lines) => {
            for (host, lines) in plan.hosts.iter_mut().zip(lines) {
                host.work.lines = lines;
            }
        }
        Err(ExpandError::Missing(missing)) => plan.missing_variables = missing.names,
        Err(ExpandError::NoSuchSnippet(name)) => return Err(PlanError::NoSuchSnippet(name)),
        // Every host that runs was just resolved.
        Err(ExpandError::NoSuchHost(_)) => unreachable!("a planned host is in the workspace"),
    }
    Ok(plan)
}

/// Plans the step of the kept run `run` again on its target, as the
/// workspace is now: hosts added to its folder since run too, hosts gone or
/// no longer runnable are skipped, and the answers it was given are
/// reused (a variable a new host lacks is missing again).
pub fn plan_again(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    run: &results::StoredRun,
) -> Result<JobPlan, PlanError> {
    plan(workspace, profiles, &run.target, &run.step)
}

/// Why the saved job `job` cannot run as the workspace is now: its
/// snippet, script, folder, saved filter or host is gone. `None`: it can be
/// planned (pre-flight may still skip every host).
pub fn saved_job_problem(workspace: &Workspace, job: &SavedJob) -> Option<PlanError> {
    let step = match &job.step {
        JobStep::Snippet { name, .. } if workspace.snippet(name).is_none() => {
            Some(PlanError::NoSuchSnippet(name.clone()))
        }
        JobStep::Script { path } => workspace.script_file(path).err().map(PlanError::from),
        _ => None,
    };
    step.or_else(|| match &job.target {
        JobTarget::Folder { path } if workspace.folder(path).is_none() => {
            Some(PlanError::NoSuchFolder(path.clone()))
        }
        JobTarget::Filter { name } if workspace.filter(name).is_none() => {
            Some(PlanError::NoSuchFilter(name.clone()))
        }
        JobTarget::Hosts { ids } => ids
            .iter()
            .find(|id| workspace.host(id).is_none())
            .map(|id| PlanError::NoSuchHost(id.clone())),
        _ => None,
    })
}

/// Plans the saved job called `name` (ignoring case) as the workspace is
/// now: its target and pre-flight list, with the answers it keeps (a
/// variable they do not fill is missing, to ask for). Refused, saying what
/// is gone, if its snippet, script, folder, saved filter or host no longer
/// exists.
pub fn plan_saved(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    name: &str,
) -> Result<(JobTarget, JobPlan), PlanError> {
    let job = workspace
        .saved_job(name)
        .ok_or_else(|| PlanError::NoSuchJob(name.trim().to_string()))?;
    if let Some(problem) = saved_job_problem(workspace, job) {
        return Err(problem);
    }
    let plan = plan(workspace, profiles, &job.target, &job.step)?;
    Ok((job.target.clone(), plan))
}

/// The hosts `target` names, each once, in order.
fn target_hosts(workspace: &Workspace, target: &JobTarget) -> Result<Vec<HostId>, PlanError> {
    match target {
        JobTarget::Folder { path } => {
            let folder = workspace
                .folder(path)
                .ok_or_else(|| PlanError::NoSuchFolder(path.clone()))?;
            let mut ids = Vec::new();
            collect_hosts(folder, &mut ids);
            Ok(ids)
        }
        JobTarget::Filter { name } => {
            let filter = workspace
                .filter(name)
                .ok_or_else(|| PlanError::NoSuchFilter(name.clone()))?;
            let shown: BTreeSet<HostId> =
                workspace.apply_filter(filter).hosts.into_iter().collect();
            let mut ids = Vec::new();
            collect_hosts(workspace.tree(), &mut ids);
            ids.retain(|id| shown.contains(id));
            Ok(ids)
        }
        JobTarget::Hosts { ids } => {
            let mut seen = BTreeSet::new();
            Ok(ids
                .iter()
                .filter(|id| seen.insert((*id).clone()))
                .cloned()
                .collect())
        }
    }
}

/// The folder's hosts, then each subfolder's, in tree order.
fn collect_hosts(folder: &Folder, ids: &mut Vec<HostId>) {
    ids.extend(folder.hosts.iter().map(|host| host.id.clone()));
    for child in &folder.folders {
        collect_hosts(child, ids);
    }
}

/// How `host` runs (its target and prompt, its lines still to fill in) and
/// the name of the definition that drives it, or why it cannot run.
fn runnable(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    host: &ResolvedHost,
    step: &JobStep,
) -> Result<(HostWork, String), SkipReason> {
    let id = &host.host.id;
    let protocol = host.settings.protocol.value;
    if protocol != Protocol::Ssh {
        return Err(SkipReason::NotSsh { protocol });
    }
    let definition = workspace
        .host_platform(id)
        .ok_or_else(|| SkipReason::NoPlatform {
            platform: host.settings.platform.as_ref().map(|p| p.value.clone()),
        })?;
    let backup = match step {
        JobStep::Snippet { .. } | JobStep::Script { .. } | JobStep::Enrich => None,
        JobStep::ConfigBackup => Some(BackupWork {
            command: given(&definition.show_config).ok_or_else(|| SkipReason::NoShowConfig {
                platform: definition.name.clone(),
            })?,
            ignored_lines: given(&definition.ignored_lines),
        }),
    };
    let profile = match profiles.login_with(id, Protocol::Ssh).map_err(skip)? {
        HostLogin::Profile { profile, .. } => profile,
        HostLogin::NotSetUp { profile } => return Err(SkipReason::ProfileNotSetUp { profile }),
        HostLogin::Prompt | HostLogin::Telnet | HostLogin::Serial => {
            return Err(SkipReason::NoProfile)
        }
    };
    let enable_secret = profiles
        .profile(&profile)
        .map_err(skip)?
        .and_then(|stored| stored.enable_secret.clone())
        .filter(|secret| !secret.is_empty())
        .map(EnableSecret);
    let jump_hosts = profiles.jump_hosts(id).map_err(skip)?;
    if let Some(jump) = jump_hosts.iter().find(|j| j.auth == SshAuth::Prompt) {
        return Err(SkipReason::JumpHostNeedsLogin {
            jump_host: jump.name.clone(),
        });
    }
    let work = HostWork {
        target: Some(profiles.profile_target(id).map_err(skip)?),
        profile: Some(profile),
        enable_secret,
        prompt: definition.prompt.clone(),
        privilege_command: given(&definition.privilege_command),
        privilege_prompt: given(&definition.privilege_prompt),
        disable_paging: given(&definition.disable_paging),
        lines: Vec::new(),
        backup,
        napalm_driver: None,
    };
    Ok((work, definition.name.clone()))
}

/// How `host` takes part in a script job (nothing to drive: the script
/// does) and its platform as written (`""` if none), or why it cannot: its
/// credential profile is named but not stored, or the store cannot say.
fn script_runnable(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    host: &ResolvedHost,
) -> Result<(HostWork, String), SkipReason> {
    if let Some(profile) = &host.settings.credential_profile {
        let name = &profile.value;
        if profiles.profile(name).map_err(skip)?.is_none() {
            return Err(SkipReason::ProfileNotSetUp {
                profile: name.clone(),
            });
        }
    }
    tunnelled(workspace, profiles, host)?;
    let work = HostWork {
        target: None,
        profile: host
            .settings
            .credential_profile
            .as_ref()
            .map(|p| p.value.clone()),
        enable_secret: None,
        prompt: String::new(),
        privilege_command: None,
        privilege_prompt: None,
        disable_paging: None,
        lines: Vec::new(),
        backup: None,
        napalm_driver: None,
    };
    let platform = host
        .settings
        .platform
        .as_ref()
        .map(|p| p.value.clone())
        .unwrap_or_default();
    Ok((work, platform))
}

/// How `host` takes part in filling in from devices (its NAPALM driver)
/// and the name of its definition, or why it cannot: it is not reached
/// over SSH, its definition has no NAPALM driver, it has no stored
/// profile, or it is behind jump hosts that cannot be tunnelled.
fn enrich_runnable(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    host: &ResolvedHost,
) -> Result<(HostWork, String), SkipReason> {
    let id = &host.host.id;
    let protocol = host.settings.protocol.value;
    if protocol != Protocol::Ssh {
        return Err(SkipReason::NotSsh { protocol });
    }
    let definition = workspace
        .host_platform(id)
        .ok_or_else(|| SkipReason::NoPlatform {
            platform: host.settings.platform.as_ref().map(|p| p.value.clone()),
        })?;
    let driver =
        crate::enrich::napalm_driver(definition).ok_or_else(|| SkipReason::NoNapalmDriver {
            platform: definition.name.clone(),
        })?;
    let profile = match profiles.login_with(id, Protocol::Ssh).map_err(skip)? {
        HostLogin::Profile { profile, .. } => profile,
        HostLogin::NotSetUp { profile } => return Err(SkipReason::ProfileNotSetUp { profile }),
        HostLogin::Prompt | HostLogin::Telnet | HostLogin::Serial => {
            return Err(SkipReason::NoProfile)
        }
    };
    tunnelled(workspace, profiles, host)?;
    let work = HostWork {
        target: None,
        profile: Some(profile),
        enable_secret: None,
        prompt: String::new(),
        privilege_command: None,
        privilege_prompt: None,
        disable_paging: None,
        lines: Vec::new(),
        backup: None,
        napalm_driver: Some(driver),
    };
    Ok((work, definition.name.clone()))
}

/// Why a script cannot reach `host` if it is behind jump hosts: the
/// chain is broken, a jump host has no stored profile (the tunnel cannot
/// log in to it), or the host is reached over telnet (only SSH is
/// tunnelled). Hosts reached directly, and serial ones, pass.
fn tunnelled(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    host: &ResolvedHost,
) -> Result<(), SkipReason> {
    let protocol = host.settings.protocol.value;
    if protocol == Protocol::Serial {
        return Ok(());
    }
    match workspace.jump_chain(host) {
        Ok(chain) if chain.is_empty() => return Ok(()),
        _ if protocol == Protocol::Telnet => {
            return Err(SkipReason::TelnetBehindJumpHost {
                jump_host: host
                    .settings
                    .jump_host
                    .as_ref()
                    .map(|jump| jump.value.clone())
                    .unwrap_or_default(),
            })
        }
        _ => {}
    }
    let jump_hosts = profiles.jump_hosts(&host.host.id).map_err(skip)?;
    match jump_hosts.iter().find(|jump| jump.auth == SshAuth::Prompt) {
        Some(jump) => Err(SkipReason::BehindJumpHost {
            jump_host: jump.name.clone(),
        }),
        None => Ok(()),
    }
}

fn skip(error: ProfileError) -> SkipReason {
    match error {
        ProfileError::JumpChain(error) => SkipReason::JumpChain {
            error: error.to_string(),
        },
        ProfileError::NotSetUp(profile) => SkipReason::ProfileNotSetUp { profile },
        other => SkipReason::Credentials {
            error: other.to_string(),
        },
    }
}
