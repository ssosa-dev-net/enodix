# Python environment, script jobs and enrichment

The app's Python setup and the jobs that run Python: script jobs and host enrichment. The package itself is in `python-bridge.md`, tunnels in `tunnels.md`.

- Python environment (#51, `src/python.rs`). Per-user choice
  `PythonChoice { interpreter, setup: Managed | Own }` in
  `<data folder>/python.yaml` (`load` → `None` if missing or damaged;
  `save`; `clear` keeps the environment on disk). `status(choice,
expected_package, wheel)` probes the interpreter each time →
  `PythonStatus::NotSet | Ready | Problem { problem }` (serde `kind`, each
  with a `message`; `NOT_SET_MESSAGE` says how to turn Python on);
  `PythonProblem::Missing | NotPython | TooOld (< 3.10) | PackageMissing |
PackageMismatch` (package version ≠ `package_version(app version)`, the
  PEP 440 rule of `build-wheel.mjs`); for `Own` the advice is
  `"<python>" -m pip install "<wheel>"`. The probe: `<program> -c
exec(bytes.fromhex(...))` (one line, so batch-file quoting cannot hurt it;
  drops `""`/`.` from `sys.path`), JSON with version, `sys.executable`,
  `enodix.__version__` or the import error; 30 s timeout (a timeout or cancel ends the process tree, see #56; since #124 through `crate::process`, below); no console
  window (`CREATE_NO_WINDOW`), cwd = temp dir; from an AppImage none of
  what points into `APPDIR` (its `PYTHONHOME` broke the system's Python,
  #114, `tests/bundle_environment.rs`). `find_python(candidates)`
  (`system_candidates()`: `py -3` on Windows, `python3`, `python`) →
  first ≥3.10 (its `sys.executable` is what is stored) or `FindError::
NoPython | TooOld`. `SetupOffer::find(candidates, data, wheel_folder,
app_version)` (Python looked for first, then the wheel `enodix-<v>-py3-none-any.whl` must exist:
  `OfferError::NoWheel`; since #128 its second argument is the
  environment's folder itself, `DataFolders::python_environment`: on
  Windows `%LOCALAPPDATA%\dev.enodix.app\python`, while `python.yaml`
  stays in the settings folder; the move, and an environment left on
  another drive, in `release-linux.md` "The data folders"; `existing`) →
  `plan(&[Library])` (`Nornir` = `nornir>=3.3,<4`, `Netmiko`, `Scrapli`,
  `Napalm`; with Nornir, `nornir-netmiko|scrapli|napalm` for the others) →
  `SetupPlan.installs` (wheel first, `InstallSource::Wheel|Index`).
  `create_environment(plan, data, expected, &SetupCanceller, progress)`:
  `<base> -m venv <env>` (in place if there), `pip install --no-index
--force-reinstall --no-deps <wheel>`, one `pip install` of the index
  packages, then checks and saves the `Managed` choice;
  `SetupProgress::Step | Output` per line; `SetupError::Failed { step,
reason } | Cancelled | NotWorking | NotSaved`. Shell (`src-tauri/src/
python.rs`, `Arc<PythonSetup>`; wheel folder = `<exe dir>/python`, which
  is `target/release/python` for `app:build` and the installer's/zip's
  `python/`): `python_status`, `python_offer` (keeps the offer),
  `python_plan { libraries }` (keeps the plan), `python_setup { onProgress }`
  (runs only the kept plan), `python_setup_cancel`,
  `python_use_interpreter { path }` (stores, installs nothing),
  `python_turn_off`; all probing runs in `spawn_blocking`; while a setup runs, use/turn-off are refused. Interface:
  `python.svelte.ts` (`python.view`, refreshed at start; **`pythonOff()`**
  = null or the message: later Python features (script jobs) must check it
  and show it instead of running), `PythonSection.svelte` in Preferences
  (`[data-python-section]`, `[data-python-status=notSet|ready|problem]`,
  `[data-python-interpreter]`, `[data-python=set-up|use-own|use|check|
turn-off]`, form `[data-python-form]` input `python-interpreter` +
  Browse…), `PythonSetup.svelte`, a view tab since #73 (view
  `python-setup`, one tab, no subject, opened by `[data-python=set-up]`
  with `openView("python-setup")`; Cancel/Close close the tab; closing
  the tab while it runs cancels the setup; `[data-view=python-setup]
[data-step=finding|offer|confirm|running|done|failed]`,
  `[data-python-base]`, `[data-python-environment]`, checkboxes
  `python-library-<id>`, `[data-python-install=<name>][data-source=wheel|
index]`, `[data-python-install-note]` (#127: the list is what is asked
  for; with index packages, what each needs comes from the index unlisted
  and nothing is pinned; the environment gets pip from venv. Chosen over a
  `pip install --dry-run --report` resolution, which would download every
  package before asking), `[data-python-output]`, `[data-python-setup=continue|back|install|
cancel]`, `[data-python-setup-result=done|failed]`). Tests:
  `tests/python_env.rs` over `support/python.rs` (`FakePython`: `.cmd` /
  sh scripts answering the probe as a given version, `failing`,
  `sleeping`; `real_python()`; `build_wheel` runs the node script); the
  real-Python tests (check + a real environment with no libraries, offline,
  ~5 s) print SKIPPED without Python 3.10+ (or Node). Smoke: `pythonSetup`
  (off message, own interpreter = the machine's Python, turn off, offer
  in one tab beside Preferences (asked again: the same tab), confirmation lists only the wheel and nothing exists yet, real setup in
  the smoke's data folder, ready) and `pythonAfterRestart`; without
  Python only the "No Python was found" message. Not verified: installing
  libraries from the index through the app (needs the network), a Python
  without `venv`/`ensurepip` (Debian), Linux/macOS.
- Script jobs (#52, `src/job/script.rs`, `src/workspace/scripts.rs`).
  `Workspace::scripts()` = every `*.py` below the root (`/` paths,
  sorted ignoring case; dot-entries, symlinked folders, `__pycache__`
  and folders holding `pyvenv.cfg` skipped; read from disk each time),
  `script_file(path)` → file or `ScriptError::BadPath` (`..`, absolute,
  dot part, not `.py`, a symlink or skipped folder on the way) | `NotFound`.
  `JobStep::Script { path }` (`{kind: script, path}`): `plan` refuses an
  unknown script (`PlanError::NoSuchScript`) and runs every target host
  except `ProfileNotSetUp` (a named profile not stored: the plugin would
  refuse to load) and store errors; telnet, no platform, no profile and
  jump hosts are the script's business (`PlannedHost.platform` = as
  written or ""). Production list as for snippets. `JobOptions.script:
Option<ScriptOptions { interpreter, cli, environment }>`; without it
  `check`/`start` give `StartError::NoPython`. `start` runs the script on
  a thread (std process, `python::quiet_command`/`stop` now
  `pub(crate)`): `<interpreter> -u <file>`, cwd = root, no input, no
  window, env `PYTHONUNBUFFERED`, `ENODIX_WORKSPACE` (root),
  `ENODIX_CLI` (if given), `ENODIX_JOB_TARGETS` = a temp
  `enodix-job-<uuid>.json` (`ScriptTargets { format 1, workspace,
hosts }`, removed when it ends) and the extra variables. stdout/stderr
  share one pipe (`io::pipe`, since #56: order kept) and one `PlainText`, streamed as `JobEvent::Output`
  under the host id `SCRIPT_OUTPUT` (`"script"`; no host id lacks
  `.yaml`); states `Waiting` → `Running` → `Done` (exit 0) | `Failed
{ reason: "the script exited with status N" }` (or could not start)
  | `Cancelled` (process tree ended, output kept). `HostResult.
exit_status` / `RunHost.exit_status` (new). Results: a script run's
  `hosts` = the script alone (`output/script.txt`), `StoredRun.targets:
[RunTarget { id, name, address }]` the hosts it ran for (skipped when
  empty); search, compare (same id across runs of one script), export,
  delete, `plan_again` unchanged. Scope in the binary:
  `ScriptTargets::from_environment()` (an unreadable file is an error,
  never "everything"); `inventory::export_within(ws, limit,
Some(&hosts))` intersects any limit, `--host` outside →
  `ExportError::OutsideJob`; `enodix-cli` (`job_scope`) refuses another
  workspace ("reads no other workspace"), `credentials` for a host
  outside the job, and checks the workspace for `secret`. Python package:
  `workspace` defaults to `ENODIX_WORKSPACE` (`enodix.job_workspace()`;
  `inventory()`, `credentials(None, id)`, `secret(None, name)`,
  `EnodixInventory()`), README "Script jobs". Shell: `workspace_scripts`;
  `job_start` takes `PythonSetup::script_options()` (probe in
  `spawn_blocking`: the Ready interpreter else the status message, the
  `enodix-cli` beside the exe, `ENODIX_DATA_DIR` = data folder so the
  binary asks this app's agent). Interface: `openScript(target, label)`
  (a job tab of kind `script`; `openJob` with a script plan too) from
  FolderDetails' `[data-folder-run-script]`, HostDetails'
  `[data-host-run-script]`, the filter bar's `[data-filter-run-script]`
  and the context menu ("Run script…"). JobTab `[data-step=script]`
  (select `job-script`, `[data-job-no-scripts]`, `[data-job-script-note]`),
  `[data-job-python-off]` (`pythonOff()`'s message, refreshed on open;
  Start disabled) with `[data-job-python-preferences]`; pre-flight rows
  "In inventory", no parallel input; running shows one
  `[data-job-result=script][data-host-state][data-exit-status]`.
  `LiveJob.exits`; RunView: `[data-run-exit-status]`,
  `[data-run-target=<id>]` rows, title "Script <path>" (`stepTitle`).
  Tests: `tests/script_jobs.rs` (stand-in interpreters
  `FakePython::running`: env, cwd, args, targets file then gone; exit 3;
  cannot start; streamed + cancel; kept run, search, run again; a real
  Python printing its targets, SKIPPED without 3.10+), cli
  `tests/inventory.rs` / `tests/credentials.rs` (`run_by_a_script_job_*`),
  `python/tests/test_script_job.py`. Smoke: `scriptJobWithoutPython`
  (before `pythonSetup`) and `pythonBridge` (after; SKIPPED without
  Python): `Jobs/bridge_check.py` for the Jobs folder (production
  confirmed) lists 3 hosts, reads the example device's SSH banner, gets
  the login's username over the agent, exit 0; run tab output, exit
  status, targets, search, Run again's pre-flight; `Jobs/bridge_slow.py`
  cancelled keeping its first line. Known limits: the scope is a guard
  against mistakes, not a sandbox (a script can unset the variable or
  call the store itself; named secrets stay readable); hosts behind jump
  hosts go through the tunnels (#60); no timeout (cancel it); a vault the app has not unlocked is refused
  (no console to ask on); the script's own host-key handling applies.
- Ending with the app (#124; found by review 3 of #112, finding 1).
  Every program the core starts for Python (a script job's interpreter,
  the setup's venv and pip steps, the interpreter check) is started by
  `process::start(&mut Command)` (crate-private `src/process.rs`; was
  `python::spawn`/`stop`) → `Tree { child, .. }`, `Tree::stop()` (was
  `python::stop`). Windows: `CREATE_SUSPENDED | CREATE_NO_WINDOW`, then a
  job object of its own with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (no
  breakaway allowed), `AssignProcessToJobObject`, then its one thread
  resumed (a Toolhelp thread snapshot, `ResumeThread`): nothing it starts
  is outside the job. `stop` = `TerminateJobObject` (no more `taskkill
  /T`, which only that fallback keeps: if the job cannot be made or
  assigned, stderr says so and the tree is as before); the `Tree`'s handle
  is the job's only one, so its processes end when the `Tree` is dropped
  and when the app's process ends in any way (the system closes the
  handle: crash, Task Manager, `TerminateProcess`). Unix: the program
  leads a session of its own (`quiet_command`'s `setsid`), and a second
  `pre_exec` forks a **keeper** into its process group before it runs (only
  when it leads its group, so the app's group is never at risk): signals
  blocked, its descriptors closed but the reading end of a close-on-exec
  pipe (`close_range`, else one by one up to the open-files limit, at most
  65536), it reads until EOF, then `kill(0, SIGKILL)` (its group, itself
  included). The writing end is only in the `Tree`: dropped, or the app
  gone however it went, the group ends; being in the group, the keeper
  keeps its id from naming another group. `stop` = `kill(-group)` as
  before. Not `PR_SET_PDEATHSIG`: it reaches only the direct child (the
  script's children would stay, in their session of their own) and fires
  when the spawning *thread* ends; the keeper works on Linux and macOS
  alike. So: a cancel ends the tree (as before); a script that exits
  leaves nothing running (what it left behind ends with its `Tree`: new;
  release-linux.md's "a script that exits normally leaves its own
  children running" no longer holds); closing the app cancels the job
  (`jobs.md`, "Ending with the app") and stops the setup
  (`SetupCanceller::wait(within)` = no `create_environment` runs with it,
  counted by a guard; the shell's `PythonSetup::cancel_running()` →
  the canceller to wait on); a crash or kill ends the tree all the same.
  Tunnels: `enodix-cli tunnel` is the script's child (Python's
  `subprocess.Popen`, same group and job), so it ends with the tree, and
  it also ends on its stdin's EOF. **Not covered**: on Windows a program
  that has something else start a process for it (a service, the task
  scheduler, WMI, an application already running that opens a document);
  on Unix a process that leaves the group (`setsid`, `setpgid`, a daemon
  that double-forks into a session of its own) and the keeper killed by
  itself (then a crash leaves the group running; a cancel still ends it);
  a script that `os.wait()`s for any child may reap the keeper when it
  ends (only at the end); a process listing of the script's children shows
  the keeper. Tests: `tests/script_jobs.rs`
  `cancelling_also_ends_the_processes_the_script_started` (rewritten: the
  tree's child and grandchild gone after Cancel; it used to watch for a
  file a leftover wrote 2 s after it started, which a busy machine let
  through when `taskkill` itself took longer to start: the flaky failure,
  now a wait on the processes), `the_processes_a_script_leaves_behind_end_
  when_it_exits`, `closing_the_app_cancels_a_running_script_and_its_kept_
  run_says_so`, `a_scripts_processes_end_when_the_app_is_killed` (the test
  binary run again as the job's owner, killed by the test from its spawn
  handle); `tests/python_env.rs` `closing_the_app_stops_a_setup_and_the_
  processes_of_its_step` and `a_setups_processes_end_when_the_app_is_
  killed` (`support/processes.rs`, `testing.md`). Without the job object
  (checked by hand: its creation forced to fail, the `taskkill` fallback
  left) the left-behind and the killed-owner tests fail and the others
  pass, so they test the job object and not the cancel. Run on Windows 11
  and in WSL (Ubuntu, `npm run test:linux`). Not run: macOS; the app
  killed in Task Manager by hand (the test kills its owner with
  `TerminateProcess`/`SIGKILL`, the same thing).
- Host enrichment (#55, `src/enrich.rs`, `python/enodix/enrich.py`).
  `JobStep::Enrich` (`{kind: enrich}`; `JobStep::runs_script()` = Script |
  Enrich) runs the wheel's own module `python -m enodix.enrich`
  (`ScriptRun::Module`, `ENRICH_MODULE`) through the script-job engine:
  one output `SCRIPT_OUTPUT` named `ENRICH_TITLE` ("Fill in from
  devices"), needs `JobOptions.script` (`NoPython`). Pre-flight
  (`enrich_runnable`): `NotSsh`, `NoPlatform`, `NoNapalmDriver { platform }`
  (`napalm_driver(def)` = the first of `NAPALM_DRIVERS` (ios, eos, junos,
  nxos_ssh, nxos, iosxr, iosxr_netconf) the definition is named or answers
  to: srlinux/frr none), `NoProfile`/`ProfileNotSetUp`, then the
  tunnel checks of #60 (`BehindJumpHost { jump_host }` = a jump host
  with no stored profile, `JumpChain`); other hosts behind jump hosts
  run through the tunnels.
  `ScriptTargets.napalm_drivers` (id → driver, omitted when empty);
  `FACTS_VARIABLE` `ENODIX_FACTS_FILE` = a temp file the script rewrites
  after each host (`{format 1, hosts: [{id, facts {vendor, model,
serial_number, os_version} | error}]}`); at the end the runner reads it
  (`CollectedFacts::read(path, plan hosts, missing reason)`: values
  trimmed, blanks dropped, non-targets ignored, unreported hosts / damaged
  file / other format → per-host `error`; cancelled → "cancelled before
  the script reported it") into `HostResult.facts`, which `RunRecorder`
  keeps as `StoredRun.facts` (run.yaml). `preview(ws, &facts, skipped:
&[RunSkipped])` / `preview_run(ws, &StoredRun)` (`EnrichError::
NotEnrichment|NoFacts`) → `EnrichPreview { hosts: [PreviewHost { id, name,
outcome: Changes|Unchanged|Skipped{reason}|Failed{reason} (serde kind),
changes: [FieldChange { field: EnrichField (vendor|model|serial_number|
os_version), kind: New|Replace, current, value }] }] }` (plan order, then
  skipped; equal values left out; writes nothing). `apply(ws, &preview,
replace: &[HostId])` compares again with the hosts as they are now, fills
  `New`, `Replace` only for ticked hosts, through `write_imported`
  (`HostWrite::Update`, one reload; OS version = custom `os_version`,
  `OS_VERSION_FIELD`) → `EnrichReport { filled, unchanged, skipped,
failed: [ReportedHost { id, name, fields, kept, reason }], reloadError }`
  (gone host → failed). Not saveable as a job (`save_job` refuses, like
  backups). Script: per host `credentials(None, id)` and the exported
  address/port/connect_timeout → `driver_arguments` (port always, Junos
  too: NETCONF on the SSH port works where 830 is closed; eos `transport:
ssh`; key → `use_keys, key_file, passphrase`; agent → `use_keys,
allow_agent`; enable secret → `secret`), 10 threads, prints one line per
  host (no secrets), exit 0 whatever the hosts did; without NAPALM every
  host fails "NAPALM is not installed in this Python ..."; run by hand
  (no variables) exit 2. Shell: `job_start` gives script options to any
  `runs_script()` step; `enrich_preview { run }` (keeps it in
  `EnrichPreviews`), `enrich_apply { run, replace }` → `Edited<
EnrichReport>` (refused unless that run was previewed last).
  Interface: `openEnrich(target, label)` (a job tab of kind `enrich`) from
  FolderDetails' `[data-folder-enrich]`, HostDetails' `[data-host-enrich]`,
  the tree's context menu; JobTab plans at once (pre-flight
  `[data-job-enrich-note]`, rows "Asked", no parallel input, no save,
  `[data-job-python-off]`), when finished `[data-job=review]` opens the
  review in the job tab's place; RunView `[data-run-action=review]` on
  ended enrich runs opens it beside the run's tab. `EnrichReview.svelte`,
  a view tab since #73 (view `enrich`, subject = the run id, so one per
  run; `openView("enrich", run)`; Cancel/Close close the tab; a
  workspace switch closes it; `[data-view=enrich][data-run]
[data-step=loading|preview|report]`, rows `[data-enrich-host=<id>]
[data-outcome]`, `[data-enrich-reason]`, `[data-enrich-change=<field>]
[data-change=new|replace]`, checkbox `[data-enrich-replace=<id>]` (hosts
  with a replacement), `[data-enrich-replace-all]`, `[data-enrich=apply|
cancel|close]`, `[data-enrich-report=filled|unchanged|skipped|failed]
[data-count]`, `[data-enrich-error]`). Tests: `tests/enrichment.rs`
  (drivers, pre-flight skips, preview, apply with/without ticks and a
  value typed after the preview, facts file parsing, a stand-in
  interpreter's run kept with the run, a script reporting nothing),
  `python/tests/test_enrich.py` (recording driver: logins/arguments per
  host, errors, no NAPALM, run by hand). Lab: `lab_enrichment`
  (`tests/lab/enrichment.rs`; needs `ENODIX_LAB_PYTHON` = a Python with
  NAPALM, else NOT CHECKED; `enodix` from `python/` via `PYTHONPATH`;
  the cli from `ENODIX_CLI` or `target/debug`, which `npm run test:lab`
  now builds first; since #60 the hosts are behind the lab's jump host
  (`jump.yaml`, profile `jump`) and the script tunnels through
  `enodix-cli tunnel`; a terminal session through the chain first trusts
  the keys into the check's data folder; an `AgentServer` over the lab
  `MemoryStore`): passes on cEOS 4.36, CSR 1000v 17.3 and cJunos Evolved
  25.4 with NAPALM 5.2 (2026-11-14), and through the lab's jump host
  (2026-11-18, #93; the check now leaves its own jump host out of the
  job's skips: it has no platform; its workspace setup moved to
  `tests/lab/scripts.rs`). Smoke `enrichment` (after
  `pythonBridge`, SKIPPED without Python): folder `Enrich` (ios host
  with typed vendor, eos host, frr skipped) without NAPALM (all failed,
  Fill in disabled), then a stand-in `napalm` package written into the
  smoke's environment's site-packages: review, apply (typed vendor kept,
  others filled, file checked), the host's own fill-in ticked (vendor
  replaced), review again from Jobs. Known limits: telnet hosts are skipped though NAPALM's ios
  could telnet; NX-OS and IOS-XR drivers not run on a device; the IOS
  `os_version` is NAPALM's whole version line.
- Edge testing (#119, 2026-11-21; the job side is in `jobs.md`).
  `tests/edge_scripts.rs`: the scripts are the test binary run again in a
  role (`support::processes`; roles `flood` (both streams in 64 KiB
  writes, `ENODIX_TEST_SIZE` bytes each), `not utf8`, `stdin`, `exit`
  (`ENODIX_TEST_CODE`), `abort`, `panic`, `stubborn` (ignores Ctrl+C on
  Windows, SIGINT/SIGTERM/SIGHUP on Unix), `connect` (holds a TCP
  connection to `ENODIX_TEST_PORT`)), each writing `child.pid`, behind a
  stand-in interpreter (`exec` on Unix, so a signal it dies of is the
  script's). Tried and seen:
  - A flood of 2 × 32 MiB with a 1 MiB limit: kept to the limit with the
    `output_cut` line, the same in the kept run, nothing after it, the
    script run to its end (its output still read); memory +6 MiB; 4.2 s.
    2 × 256 MiB at the default 32 MiB (`#[ignore]`d): 28 s, +102 MiB.
    **Fixed** with the jobs' output limit (`jobs.md`): the whole stream was
    kept in memory, in the events and in the kept run; and the pipe's
    reader thread passed it on through an unbounded channel, so a script
    writing faster than the text was taken piled it up: now a bounded one
    (`PIECES_IN_FLIGHT` 64 pieces of 8 KiB), the script waiting on its
    pipe.
  - Cancelling a flood (ended within 10 s, process gone), a script that
    ignores every request to stop (gone: the job object or the group's
    SIGKILL does not ask), one holding a connection open as a tunnel or
    the credential agent's would be (the far end sees it close, process
    gone).
  - Every end: exit 0, 1, 2, 255 (`exit_status` and the reason), a panic
    (101 and its message), an abort on Unix ("ended without an exit status
    (by a signal)"). Not played on Windows: a crashed program's negative
    status, which the stand-in's `cmd.exe` turns into 0 (the app starts
    `python.exe` itself, whose status comes through as it is).
  - Standard input: the script reads end of file at once (`Stdio::null`)
    and goes on. Text that is not UTF-8: replacement characters, a
    character split across two writes 200 ms apart kept whole, the kept
    run's file UTF-8.
  - A package the script needs missing (real Python, SKIPPED without):
    failed "exited with status 1", Python's `ModuleNotFoundError` in the
    output.
  - Already covered: a hang cancelled (`script_jobs.rs`, streamed and
    cancelled), children left behind and the app killed (#124), the
    interpreter gone before the start ("could not be started"), the Python
    environment missing, gone (managed: "environment is gone"), not Python,
    too old, without the package or with another version
    (`python_env.rs`: `status` probes each time, so one deleted under the
    app shows at the next check); the tunnel ending with its script
    (`python/tests/test_tunnels.py`). Not possible: deleting an interpreter
    while it runs (Windows refuses; Unix keeps the running file).
  - Enrichment (`python/tests/test_enrich_edges.py`, drivers that
    misbehave): an answer that is not a mapping (`None`, a list, text)
    fails the host naming the error; values of other types kept as text,
    blank ones dropped, `{}` is "no facts reported"; a `KeyError` from the
    driver is its words; a close that fails keeps the facts; twelve devices
    taking 0.4 s each are asked ten at once and each kept as it answers; a
    device that does not answer fails with NAPALM's timeout, which is the
    host's connect timeout (the inventory always exports one: 15 s by
    default, so NAPALM's own 60 s never applies). **Fixed**: on Windows
    the facts file could not be replaced while anything read it
    (`PermissionError` from `os.replace`, seen with the test reading it):
    the script died and every host's facts were lost; `_write` now tries
    again for up to 2 s (`REPLACE_TRIES`). Known limit: a driver that
    ignores its timeout hangs the job until it is cancelled (no job
    timeout for scripts). Also fixed on the way: `npm run test:python`
    built only the example server (`--example` with two packages builds
    only that example), so the binary had to have been built by something
    else first.

- Security review (2026-11-22): the app's own module (`ScriptRun::Module`,
  enrichment's `-m enodix.enrich`) runs with the interpreter's folder as
  its current folder, not the workspace's (`-m` puts the current folder
  first on the import path; a workspace's own `enodix` package would
  replace the app's). Workspace scripts still run in the workspace. The
  targets file is created 0600 on Unix (`write_private`). The Python
  package's `find_cli` searches `PATH` itself (`_on_path`), never the
  current folder, for the binary's own name only.

- Security review (2026-11-22): `enodix-cli inventory` gives each host
  `host_keys` (the keys `KnownHosts::trusted(address, port)` gives, as
  `algorithm base64`; none for a serial host or an unreadable file, with a
  warning). `_tunnel.direct_host_keys(host)` writes a known-hosts file and
  its ssh_config for a direct host (mkdtemp, removed at exit) or gives
  `None`. `enodix.enrich` refuses a direct host it gives `None` for,
  before fetching the login, and passes `host_key_options` for the
  Netmiko drivers either way; pre-flight (`job.rs`, Enrich) now records the
  device's own key place for a direct host ("Fill in from devices will
  refuse this host"). The Nornir plugin applies them to direct SSH hosts
  too, sets `enodix.host_key_checked`, and warns naming the unchecked ones
  (it does not refuse them: a decision left open). The cli tests give
  `ENODIX_DATA_DIR` an empty folder (`run`, `run_with_data`).
