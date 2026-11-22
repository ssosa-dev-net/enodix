//! The Python environment's commands (#51): a thin adapter over
//! `netterm_core::python`. The setup runs exactly the plan the user
//! confirmed (kept here between `python_plan` and `python_setup`), on a
//! blocking thread, its progress sent on a channel.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use netterm_core::data_folders::{self, DataFolders};
use netterm_core::job::ScriptOptions;
use netterm_core::python::{
    self, create_environment, package_version, status, system_candidates, Library, PythonChoice,
    PythonStatus, Setup, SetupCanceller, SetupOffer, SetupPlan, SetupProgress,
};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

/// Where the app keeps the choice and its environment, where its wheel is,
/// and the setup under way.
pub struct PythonSetup {
    /// The choice is kept here (the settings folder).
    data_folder: PathBuf,
    /// Where the app's environment is made (the local data folder's, since
    /// #128), and one left behind in the roaming folder.
    folders: DataFolders,
    /// `python` in the resource folder: beside the program on Windows (the
    /// installer and the portable zip put the wheel there, and `app:build`
    /// leaves it in `target/release/python`), `/usr/lib/<product>/python`
    /// in a Linux package.
    wheel_folder: PathBuf,
    /// Where the program is: the headless binary is beside it.
    program_folder: PathBuf,
    app_version: String,
    offer: Mutex<Option<SetupOffer>>,
    plan: Mutex<Option<SetupPlan>>,
    /// The setup under way; while it runs the choice is not changed
    /// otherwise (its lock is held while the choice is saved).
    running: Mutex<Option<SetupCanceller>>,
}

const SETUP_RUNNING: &str = "the app's environment is being set up; wait for it or cancel it";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PythonSetup {
    pub fn new(
        folders: DataFolders,
        program_folder: PathBuf,
        resource_folder: PathBuf,
        app_version: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            data_folder: folders.settings.clone(),
            folders,
            wheel_folder: resource_folder.join("python"),
            program_folder,
            app_version,
            offer: Mutex::new(None),
            plan: Mutex::new(None),
            running: Mutex::new(None),
        })
    }

    fn expected(&self) -> String {
        package_version(&self.app_version)
    }

    fn wheel(&self) -> Option<PathBuf> {
        Some(
            self.wheel_folder
                .join(python::wheel_name(&self.app_version)),
        )
        .filter(|w| w.is_file())
    }

    /// How a script job runs its script: the chosen interpreter if it is
    /// ready (else the message saying what is wrong), the headless binary
    /// beside the program, and this app's data folder (so that the binary
    /// asks this app for credentials). Probes the interpreter: blocks.
    pub fn script_options(&self) -> Result<ScriptOptions, String> {
        let interpreter = match self.view().status {
            PythonStatus::Ready { interpreter, .. } => interpreter,
            other => return Err(other.message().to_string()),
        };
        let cli = self
            .program_folder
            .join(format!("netterm-cli{}", std::env::consts::EXE_SUFFIX));
        Ok(ScriptOptions {
            interpreter,
            cli: cli.is_file().then_some(cli),
            environment: vec![(
                crate::DATA_DIR_VARIABLE.into(),
                self.data_folder.clone().into(),
            )],
        })
    }

    fn view(&self) -> PythonView {
        let wheel = self.wheel();
        PythonView {
            status: status(
                PythonChoice::load(&self.data_folder).as_ref(),
                &self.expected(),
                wheel.as_deref(),
            ),
            environment: self.folders.python_environment.clone(),
            wheel,
        }
    }
}

/// The chosen interpreter as it is now, with where the app's environment
/// goes and the shipped wheel.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonView {
    status: PythonStatus,
    environment: PathBuf,
    wheel: Option<PathBuf>,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn python_status(setup: State<'_, Arc<PythonSetup>>) -> Result<PythonView, String> {
    let setup = setup.inner().clone();
    blocking(move || Ok(setup.view())).await
}

/// Looks for a Python for the app's environment. Installs nothing.
#[tauri::command]
pub async fn python_offer(setup: State<'_, Arc<PythonSetup>>) -> Result<SetupOffer, String> {
    let setup = setup.inner().clone();
    blocking(move || {
        let offer = SetupOffer::find(
            &system_candidates(),
            &setup.folders.python_environment,
            &setup.wheel_folder,
            &setup.app_version,
        )
        .map_err(|error| error.to_string())?;
        *lock(&setup.offer) = Some(offer.clone());
        Ok(offer)
    })
    .await
}

/// Everything the setup with these libraries will install, kept for
/// `python_setup`.
#[tauri::command]
pub fn python_plan(
    setup: State<'_, Arc<PythonSetup>>,
    libraries: Vec<Library>,
) -> Result<SetupPlan, String> {
    let plan = lock(&setup.offer)
        .as_ref()
        .ok_or("look for a Python first")?
        .plan(&libraries);
    *lock(&setup.plan) = Some(plan.clone());
    Ok(plan)
}

/// Runs the plan the user confirmed, and only that one.
#[tauri::command]
pub async fn python_setup(
    setup: State<'_, Arc<PythonSetup>>,
    on_progress: Channel<SetupProgress>,
) -> Result<PythonView, String> {
    let setup = setup.inner().clone();
    let canceller = SetupCanceller::new();
    let plan = {
        let mut running = lock(&setup.running);
        if running.is_some() {
            return Err("a setup is already running".into());
        }
        let plan = lock(&setup.plan)
            .take()
            .ok_or("there is no confirmed setup to run")?;
        *running = Some(canceller.clone());
        plan
    };
    blocking(move || {
        // `running` is cleared however this ends.
        let _done = Running(&setup);
        create_environment(
            &plan,
            &setup.data_folder,
            &setup.expected(),
            &canceller,
            &mut |progress| {
                let _ = on_progress.send(progress);
            },
        )
        .map_err(|error| error.to_string())?;
        // An environment an older version left in the roaming folder, on
        // another drive (#128), goes now that its successor is ready.
        if let Err(error) = data_folders::remove_left_environment(&setup.folders) {
            eprintln!("the old Python environment could not be removed: {error}");
        }
        Ok(setup.view())
    })
    .await
}

/// Clears the running setup when dropped.
struct Running<'a>(&'a PythonSetup);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        *lock(&self.0.running) = None;
    }
}

impl PythonSetup {
    /// Cancels the setup under way, if one is: its canceller, to wait on.
    pub fn cancel_running(&self) -> Option<SetupCanceller> {
        let running = lock(&self.running).clone()?;
        running.cancel();
        Some(running)
    }
}

#[tauri::command]
pub fn python_setup_cancel(setup: State<'_, Arc<PythonSetup>>) {
    setup.cancel_running();
}

/// Stores the user's own interpreter (installs nothing) and checks it.
#[tauri::command]
pub async fn python_use_interpreter(
    setup: State<'_, Arc<PythonSetup>>,
    path: PathBuf,
) -> Result<PythonView, String> {
    let setup = setup.inner().clone();
    blocking(move || {
        if path.as_os_str().is_empty() {
            return Err("give the interpreter's path".into());
        }
        let running = lock(&setup.running);
        if running.is_some() {
            return Err(SETUP_RUNNING.into());
        }
        PythonChoice {
            interpreter: path,
            setup: Setup::Own,
        }
        .save(&setup.data_folder)
        .map_err(|error| format!("the choice could not be saved: {error}"))?;
        drop(running);
        Ok(setup.view())
    })
    .await
}

/// Forgets the choice: the Python features are off again. The app's
/// environment stays on disk.
#[tauri::command]
pub async fn python_turn_off(setup: State<'_, Arc<PythonSetup>>) -> Result<PythonView, String> {
    let setup = setup.inner().clone();
    blocking(move || {
        let running = lock(&setup.running);
        if running.is_some() {
            return Err(SETUP_RUNNING.into());
        }
        PythonChoice::clear(&setup.data_folder)
            .map_err(|error| format!("the choice could not be removed: {error}"))?;
        drop(running);
        Ok(setup.view())
    })
    .await
}
