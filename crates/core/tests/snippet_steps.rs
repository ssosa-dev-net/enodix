//! A snippet's steps (#23): read from and written to the snippets file,
//! filled for a host, and run on an open session over SSH, telnet and
//! serial, as the device sees it: each line sent only once the device is
//! back at its prompt, questions answered, the run stopped at an error, a
//! timeout, the user's stop or input, or the session's end.

mod support;

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use enodix_core::credentials::Profiles;
use enodix_core::job::{self, JobStep, JobTarget, PlanError};
use enodix_core::layout::InputOrigin;
use enodix_core::paste::{confirms_steps, PasteSafety};
use enodix_core::session::{
    OutputOptions, SerialParameters, SerialTarget, SessionEvent, SessionEvents, SessionId,
    SessionManager, SessionState, SessionTarget, TerminalSize,
};
use enodix_core::steps::{
    CalledOff, Permission, RunEnd, RunEvent, StartError, Step, StepRuns, Steps, StepsPlan, Wait,
    DEFAULT_PROMPT,
};
use enodix_core::template::Variables;
use enodix_core::workspace::{Environment, ExpandError, HostId, Snippet, Workspace, SNIPPETS_FILE};
use support::serial::VirtualSerialPorts;
use support::telnet::{TestTelnetServer, PASSWORD_PROMPT, TELNET_PROMPT, USERNAME_PROMPT};
use support::{
    device_prompt, known_hosts, next_state, output_ending_with, unprivileged_prompt, TestSshServer,
    ENABLE_SECRET, INVALID_INPUT, PASSWORD, USERNAME, WAIT,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

// --- The snippets file ---

const SAVE_CONFIG: &str = r#"Save the configuration:
  group: Changes
  timeout: 60
  answer:
    "Destination filename": ""
    "[confirm]": "{{confirm}}"
  stop:
    - "% Invalid input"
    - /% Incomplete.*/
  steps:
    - copy running-config startup-config
    - send: reload in {{minutes}}
      wait: /Proceed with reload\?/
      timeout: 10
    - send: "y"
      wait: none
    - wait: prompt
    - wait: Press RETURN
Show version: show version
"#;

fn save_config() -> Steps {
    Steps::new(vec![
        Step::send("copy running-config startup-config"),
        Step::send("reload in {{minutes}}")
            .waiting(Wait::For(r"/Proceed with reload\?/".into()))
            .within(10),
        Step::send("y").waiting(Wait::Nothing),
        Step {
            send: None,
            wait: Wait::Prompt,
            timeout_secs: None,
        },
        Step::wait_for("Press RETURN"),
    ])
    .answering("Destination filename", "")
    .answering("[confirm]", "{{confirm}}")
    .stopping_at("% Invalid input")
    .stopping_at("/% Incomplete.*/")
    .within(60)
}

/// A workspace whose snippets file holds `snippets`, with a host `r1` on
/// the platform `platform` (none: no platform).
fn workspace_with(snippets: &str, platform: Option<&str>) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let platform = platform.map_or(String::new(), |name| format!("platform: {name}\n"));
    fs::write(
        root.join("r1.yaml"),
        format!("address: 10.0.0.1\nsite: MAD1\n{platform}"),
    )
    .unwrap();
    fs::write(root.join(SNIPPETS_FILE), snippets).unwrap();
    let workspace = Workspace::open(root).unwrap();
    (temp, workspace)
}

fn r1() -> HostId {
    HostId::new("r1.yaml")
}

#[test]
fn steps_are_read_from_the_snippets_file() {
    let (_temp, workspace) = workspace_with(SAVE_CONFIG, None);
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    let snippet = workspace.snippet("save the configuration").unwrap();
    assert_eq!(snippet.group.as_deref(), Some("Changes"));
    assert_eq!(snippet.text, "");
    assert_eq!(snippet.steps.as_ref(), Some(&save_config()));
    // The variables of the lines sent and of the answers, in order.
    assert_eq!(snippet.variables(), ["minutes", "confirm"]);
    // A plain snippet beside it keeps its text.
    let plain = workspace.snippet("Show version").unwrap();
    assert_eq!(plain.steps, None);
    assert_eq!(plain.text, "show version");
    // Found by what the steps say.
    let found: Vec<&str> = workspace
        .find_snippets("reload")
        .into_iter()
        .map(|snippet| snippet.name.as_str())
        .collect();
    assert_eq!(found, ["Save the configuration"]);
}

#[test]
fn saved_steps_read_back_the_same() {
    let (temp, mut workspace) = workspace_with("Show version: show version\n", None);
    let snippet = Snippet::with_steps("Save the configuration", save_config()).in_group("Changes");
    workspace.save_snippet(None, &snippet).unwrap();
    // Also a step waiting for a pattern spelled as a keyword.
    let keyword = Snippet::with_steps(
        "Keyword",
        Steps::new(vec![Step::send("x").waiting(Wait::For("prompt".into()))]),
    );
    workspace.save_snippet(None, &keyword).unwrap();

    let reopened = Workspace::open(temp.path()).unwrap();
    assert!(reopened.problems().is_empty(), "{:?}", reopened.problems());
    assert_eq!(
        reopened.snippet("Save the configuration").unwrap(),
        &snippet
    );
    assert_eq!(
        reopened
            .snippet("Keyword")
            .unwrap()
            .steps
            .as_ref()
            .unwrap()
            .steps[0]
            .wait,
        Wait::For("/prompt/".into())
    );
    // A line that waits for the prompt is written as the line alone.
    let file = fs::read_to_string(temp.path().join(SNIPPETS_FILE)).unwrap();
    assert!(
        file.contains("- copy running-config startup-config\n"),
        "{file}"
    );
    // Renaming keeps the steps.
    let renamed = Snippet {
        name: "Save it".into(),
        ..snippet.clone()
    };
    workspace
        .save_snippet(Some("Save the configuration"), &renamed)
        .unwrap();
    let reopened = Workspace::open(temp.path()).unwrap();
    assert_eq!(reopened.snippet("Save it").unwrap().steps, snippet.steps);
}

#[test]
fn a_snippet_with_steps_comes_back_from_the_interface_as_it_went() {
    // The editor saves a snippet's name and group with its steps as it
    // was given them (camelCase over IPC).
    let snippet = Snippet::with_steps("Save", save_config());
    let json = serde_json::to_value(&snippet).unwrap();
    assert_eq!(json["steps"]["timeoutSecs"], 60);
    assert_eq!(json["steps"]["steps"][0]["wait"]["kind"], "prompt");
    assert_eq!(json["steps"]["steps"][2]["wait"]["kind"], "nothing");
    assert_eq!(
        json["steps"]["steps"][1]["wait"],
        serde_json::json!({"kind": "for", "pattern": r"/Proceed with reload\?/"})
    );
    let back: Snippet = serde_json::from_value(json).unwrap();
    assert_eq!(back, snippet);
    // A plain snippet has no `steps` at all.
    let plain = serde_json::to_value(Snippet::new("Show", "show version")).unwrap();
    assert!(plain.get("steps").is_none());
}

#[test]
fn badly_written_steps_are_problems_and_left_out() {
    let cases = [
        ("both: {text: a, steps: [b]}\n", "both `text` and `steps`"),
        ("neither: {group: x}\n", "missing `text`"),
        ("empty: {steps: []}\n", "it has no steps"),
        (
            "odd key: {steps: [{send: a, then: b}]}\n",
            "unknown key `then`",
        ),
        (
            "idle: {steps: [{timeout: 5}]}\n",
            "neither `send` nor `wait`",
        ),
        (
            "bad regex: {steps: [{send: a, wait: /(/}]}\n",
            "cannot be used",
        ),
        (
            "no wait at all: {steps: [{send: a, wait: none}, {wait: none}]}\n",
            "step 2",
        ),
        ("zero: {timeout: 0, steps: [a]}\n", "timeout must be from 1"),
        ("text with stop: {text: a, stop: x}\n", "go with `steps`"),
        (
            "bad answer: {answer: [x], steps: [a]}\n",
            "`answer` must map",
        ),
    ];
    for (file, expected) in cases {
        let (_temp, workspace) = workspace_with(file, None);
        assert!(workspace.snippets().is_empty(), "{file}");
        let problems = format!("{:?}", workspace.problems());
        assert!(problems.contains(expected), "{file}: {problems}");
    }
}

#[test]
fn saving_steps_that_cannot_run_is_refused() {
    let (_temp, mut workspace) = workspace_with("Show version: show version\n", None);
    let bad = Snippet::with_steps("Bad", Steps::new(vec![Step::wait_for("/[unclosed/")]));
    let error = workspace.save_snippet(None, &bad).unwrap_err();
    assert!(error.to_string().contains("cannot be used"), "{error}");
}

#[test]
fn steps_are_filled_from_the_host_and_answers_and_wait_for_its_platform_prompt() {
    let file = "Ping: {steps: ['ping {{address}} source {{source}}'], answer: {'Password:': '{{secret}}'}}\n";
    let (_temp, workspace) = workspace_with(file, Some("cisco_ios"));
    let missing = workspace
        .plan_snippet_steps("ping", Some(&r1()), &Variables::new())
        .unwrap_err();
    let ExpandError::Missing(missing) = missing else {
        panic!("expected missing variables, got {missing:?}");
    };
    assert_eq!(missing.names, ["source", "secret"]);

    let answers: Variables = [("source", "lo0"), ("secret", "s3cret")]
        .into_iter()
        .collect();
    let plan = workspace
        .plan_snippet_steps("ping", Some(&r1()), &answers)
        .unwrap();
    assert_eq!(plan.lines(), ["ping 10.0.0.1 source lo0"]);
    assert_eq!(
        plan.prompt(),
        workspace.host_platform(&r1()).unwrap().prompt
    );
    // The preview names each rule (with what it answers: the user checks
    // it), then each step.
    assert_eq!(
        plan.preview(),
        [
            "Whenever \u{201c}Password:\u{201d} shows: answer \u{201c}s3cret\u{201d}",
            "1. Send \u{201c}ping 10.0.0.1 source lo0\u{201d}, wait for the prompt (up to 30 s)",
        ]
    );
    // No host (a quick connection): the default prompt, only answers.
    let answers: Variables = [("address", "192.0.2.1"), ("source", "lo0"), ("secret", "")]
        .into_iter()
        .collect();
    let plan = workspace
        .plan_snippet_steps("ping", None, &answers)
        .unwrap();
    assert_eq!(plan.prompt(), DEFAULT_PROMPT);
}

#[test]
fn a_snippet_with_steps_is_never_sent_as_lines() {
    let (_temp, workspace) = workspace_with(SAVE_CONFIG, None);
    let answers = Variables::new();
    assert_eq!(
        workspace.expand_snippet("Save the configuration", None, &answers),
        Err(ExpandError::HasSteps("Save the configuration".into()))
    );
    // Nor to the multi-send group.
    assert_eq!(
        workspace.expand_snippet_for_each("Save the configuration", &[None, None], &answers),
        Err(ExpandError::HasSteps("Save the configuration".into()))
    );
    // And a plain snippet has no steps to run.
    assert!(matches!(
        workspace.plan_snippet_steps("Show version", None, &answers),
        Err(ExpandError::NoSteps(_))
    ));
}

#[test]
fn steps_are_confirmed_as_a_paste_and_always_on_production() {
    let lab = PasteSafety {
        environment: Some(Environment::Lab),
        line_delay_ms: 0,
    };
    let production = PasteSafety {
        environment: Some(Environment::Production),
        line_delay_ms: 0,
    };
    assert!(confirms_steps(&lab, true));
    assert!(!confirms_steps(&lab, false));
    assert!(confirms_steps(&production, false));
    assert!(confirms_steps(&PasteSafety::default(), true));
}

#[test]
fn a_job_never_runs_a_snippet_with_steps() {
    let lab = support::jobs::Lab::new(&[
        (SNIPPETS_FILE, SAVE_CONFIG.to_string()),
        ("r1.yaml", support::jobs::host_at(22, "cisco_ios", "")),
    ]);
    let profiles = Profiles::new(&lab.store, &lab.workspace);
    let step = JobStep::Snippet {
        name: "Save the configuration".into(),
        answers: Default::default(),
    };
    let refused = job::plan(
        &lab.workspace,
        &profiles,
        &JobTarget::Folder { path: "".into() },
        &step,
    );
    assert!(
        matches!(&refused, Err(PlanError::SnippetHasSteps(name)) if name == "Save the configuration"),
        "{refused:?}"
    );
    // A snippet of lines still plans.
    let lines = JobStep::Snippet {
        name: "Show version".into(),
        answers: Default::default(),
    };
    assert!(job::plan(
        &lab.workspace,
        &profiles,
        &JobTarget::Folder { path: "".into() },
        &lines
    )
    .is_ok());
}

#[test]
fn the_steps_run_are_those_shown() {
    let answers: Variables = [("minutes", "5"), ("confirm", "")].into_iter().collect();
    let shown = |snippets: &str, platform: Option<&str>, answers: &Variables| {
        let (_temp, workspace) = workspace_with(snippets, platform);
        workspace
            .plan_snippet_steps("Save the configuration", Some(&r1()), answers)
            .unwrap()
    };
    let checked = shown(SAVE_CONFIG, None, &answers);
    // The same snippet for the same host is the same run.
    assert_eq!(
        shown(SAVE_CONFIG, None, &answers).digest(),
        checked.digest()
    );
    let manager = SessionManager::new(known_hosts());
    let runs = StepRuns::new();
    let refused =
        |now: StepsPlan| runs.start(&manager, SessionId(1), now, &allowed(&checked), |_| {});
    // The file changed since: another line.
    let edited = SAVE_CONFIG.replace("reload in", "reload at");
    assert_eq!(
        refused(shown(&edited, None, &answers)).unwrap_err(),
        StartError::Changed
    );
    // Another answer.
    let other: Variables = [("minutes", "50"), ("confirm", "")].into_iter().collect();
    assert_eq!(
        refused(shown(SAVE_CONFIG, None, &other)).unwrap_err(),
        StartError::Changed
    );
    // The host's platform, and so its prompt.
    assert_eq!(
        refused(shown(SAVE_CONFIG, Some("cisco_ios"), &answers)).unwrap_err(),
        StartError::Changed
    );
    // Another stop pattern.
    let stops = SAVE_CONFIG.replace("% Invalid input", "% Bad input");
    assert_eq!(
        refused(shown(&stops, None, &answers)).unwrap_err(),
        StartError::Changed
    );
    assert_eq!(
        StartError::Changed.to_string(),
        "the snippet changed since you checked it, so nothing was sent; look again"
    );
}

// --- Running on a session ---

/// Everything the session showed, as its consumer keeps acknowledging it.
#[derive(Clone, Default)]
struct Shown(Arc<Mutex<Vec<u8>>>);

impl Shown {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Takes the session's events as a window would: output acknowledged and
/// kept, until the session closes.
fn consume(mut events: SessionEvents) -> Shown {
    let shown = Shown::default();
    let kept = shown.clone();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if let SessionEvent::Output(bytes) = event {
                events.acknowledge(bytes.len());
                kept.0.lock().unwrap().extend_from_slice(&bytes);
            }
        }
    });
    shown
}

async fn connected(
    manager: &SessionManager,
    target: SessionTarget,
    first: &[u8],
) -> (SessionId, Shown) {
    let (id, mut events) = manager.open(target, SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    if !first.is_empty() {
        output_ending_with(&mut events, first).await;
    }
    (id, consume(events))
}

/// A session to a device-mode SSH server called `r1`, at its prompt.
async fn device(unprivileged: bool) -> (TestSshServer, SessionManager, SessionId, Shown) {
    let server = if unprivileged {
        TestSshServer::start_unprivileged_device("r1").await
    } else {
        TestSshServer::start_device("r1").await
    };
    let manager = SessionManager::new(known_hosts());
    let prompt = if unprivileged {
        unprivileged_prompt("r1")
    } else {
        device_prompt("r1")
    };
    let (id, shown) = connected(&manager, SessionTarget::Ssh(server.target()), &prompt).await;
    (server, manager, id, shown)
}

fn plan(steps: Steps) -> StepsPlan {
    steps.fill(&Variables::new(), None).unwrap()
}

/// Starts `plan` on `session`; its events arrive on the receiver.
fn start(
    runs: &StepRuns,
    manager: &SessionManager,
    session: SessionId,
    plan: StepsPlan,
) -> mpsc::UnboundedReceiver<RunEvent> {
    let permission = allowed(&plan);
    let (report, events) = mpsc::unbounded_channel();
    runs.start(manager, session, plan, &permission, move |event| {
        let _ = report.send(event);
    })
    .unwrap();
    events
}

/// What lets `plan` run: shown to the user as it is, and confirmed.
fn allowed(plan: &StepsPlan) -> Permission {
    Permission {
        safety: PasteSafety::default(),
        confirm_multi_line: true,
        previewed: plan.digest(),
        confirmed: true,
        in_group: false,
    }
}

/// Starting `plan` on `session` though it would be allowed: what the start
/// says.
fn start_refused(
    runs: &StepRuns,
    manager: &SessionManager,
    session: SessionId,
    plan: StepsPlan,
) -> Result<(), StartError> {
    let permission = allowed(&plan);
    runs.start(manager, session, plan, &permission, |_| {})
}

/// Every event up to the end, and the end.
async fn ended(events: &mut mpsc::UnboundedReceiver<RunEvent>) -> (Vec<RunEvent>, RunEnd) {
    let mut seen = Vec::new();
    tokio::time::timeout(WAIT * 2, async {
        while let Some(event) = events.recv().await {
            if let RunEvent::Ended { end, message } = event {
                assert_eq!(message, end.message());
                return (seen, end);
            }
            seen.push(event);
        }
        panic!("the run ended without saying how; it told {seen:?}");
    })
    .await
    .expect("the run did not end in time")
}

/// Where `needle` first shows in `haystack`, which must hold it.
fn at(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in {haystack:?}"))
}

#[tokio::test]
async fn each_line_waits_for_the_device_to_be_back_at_its_prompt() {
    let (_server, manager, id, shown) = device(false).await;
    let runs = StepRuns::new();
    // `drip` shows its lines 150 ms apart, then the prompt.
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send("drip 3 150"),
            Step::send("echo second"),
        ])),
    );
    let (told, end) = ended(&mut events).await;
    assert_eq!(end, RunEnd::Finished);
    assert!(!runs.running(id));
    let steps: Vec<usize> = told
        .iter()
        .filter_map(|event| match event {
            RunEvent::Step {
                index, count: 2, ..
            } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(steps, [0, 1]);
    // Typed only after the last line and the prompt: a line sent at once
    // would have been echoed among them.
    let text = shown.text();
    assert!(at(&text, "line 3\r\nr1# echo second") > 0, "{text:?}");
    assert!(text.ends_with("second\r\nr1# "), "{text:?}");
}

#[tokio::test]
async fn a_question_is_answered_when_it_shows() {
    let (_server, manager, id, shown) = device(true).await;
    let runs = StepRuns::new();
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(
            Steps::new(vec![
                Step::send("enable"),
                Step::send("show running-config"),
            ])
            .answering("Password:", ENABLE_SECRET)
            .stopping_at("% Invalid input"),
        ),
    );
    let (told, end) = ended(&mut events).await;
    assert_eq!(end, RunEnd::Finished, "{}", shown.text());
    assert!(
        told.contains(&RunEvent::Answered {
            index: 0,
            question: "Password:".into(),
        }),
        "{told:?}"
    );
    // The secret got it to the privileged prompt, where the configuration
    // shows: no error to stop at.
    let text = shown.text();
    assert!(text.contains("hostname r1"), "{text:?}");
    assert!(!text.contains(INVALID_INPUT), "{text:?}");
}

#[tokio::test]
async fn a_stop_pattern_stops_the_run_naming_the_step_and_line() {
    let (_server, manager, id, shown) = device(true).await;
    let runs = StepRuns::new();
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(
            Steps::new(vec![
                Step::send("show running-config"),
                Step::send("echo never sent"),
            ])
            .stopping_at("% Invalid input"),
        ),
    );
    let (_, end) = ended(&mut events).await;
    assert_eq!(
        end,
        RunEnd::Stopped {
            step: 0,
            line: Some("show running-config".into()),
            pattern: "% Invalid input".into(),
            shown: INVALID_INPUT.into(),
        }
    );
    assert!(end.message().contains("at step 1"), "{}", end.message());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!shown.text().contains("never sent"), "{}", shown.text());
}

#[tokio::test]
async fn the_echo_of_the_line_sent_is_not_taken_for_the_device_s_answer() {
    let (_server, manager, id, _shown) = device(false).await;
    let runs = StepRuns::new();
    // The line holds the stop pattern; the device's answer (its hostname)
    // does not.
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![Step::send("show hostname boom")]).stopping_at("boom")),
    );
    assert_eq!(ended(&mut events).await.1, RunEnd::Finished);
}

#[tokio::test]
async fn a_step_can_wait_for_a_pattern_of_its_own() {
    let (_server, manager, id, shown) = device(false).await;
    let runs = StepRuns::new();
    let started = Instant::now();
    // `later` gives the prompt at once and its text a second later.
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send("later 1 ready now").waiting(Wait::For("ready now".into())),
            Step::send("echo next"),
        ])),
    );
    assert_eq!(ended(&mut events).await.1, RunEnd::Finished);
    assert!(started.elapsed() >= Duration::from_secs(1));
    let text = shown.text();
    assert!(
        at(&text, "ready now\r\n") < at(&text, "echo next"),
        "{text:?}"
    );
}

#[tokio::test]
async fn a_wait_that_passes_its_timeout_stops_the_run() {
    let (_server, manager, id, shown) = device(false).await;
    let runs = StepRuns::new();
    let started = Instant::now();
    // `hang` never gives the prompt back.
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send("hang").within(1),
            Step::send("echo never sent"),
        ])),
    );
    let (_, end) = ended(&mut events).await;
    assert_eq!(
        end,
        RunEnd::TimedOut {
            step: 0,
            line: Some("hang".into()),
            waiting_for: "the prompt (up to 1 s)".into(),
        }
    );
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!shown.text().contains("never sent"));
}

#[tokio::test]
async fn the_user_s_input_stops_the_run_and_is_not_sent() {
    let (_server, manager, id, shown) = device(false).await;
    let runs = StepRuns::new();
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send("drip 1 2000"),
            Step::send("echo never sent"),
        ])),
    );
    // Its first step is under way.
    assert!(matches!(
        events.recv().await,
        Some(RunEvent::Step { index: 0, .. })
    ));
    assert!(runs.running(id));
    // The terminal's replies to the device go on, and do not stop it.
    assert_eq!(runs.user_input(vec![id], InputOrigin::Reply), [id]);
    assert!(runs.running(id));
    // A second run on the same session is refused meanwhile.
    let refused = start_refused(&runs, &manager, id, plan(Steps::new(vec![Step::send("x")])));
    assert_eq!(refused.unwrap_err(), StartError::AlreadyRunning);
    // A key typed stops it, and goes nowhere; another session it was
    // typed to (multi-send) still gets it.
    let other = SessionId(id.0 + 1000);
    assert_eq!(
        runs.user_input(vec![id, other], InputOrigin::Typed),
        [other]
    );
    let (_, end) = ended(&mut events).await;
    assert_eq!(
        end,
        RunEnd::CalledOff {
            step: 0,
            line: Some("drip 1 2000".into()),
            by: CalledOff::Input,
        }
    );
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert!(!shown.text().contains("never sent"));
    // Once ended, the session takes steps again.
    let mut again = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![Step::send("echo again")])),
    );
    assert_eq!(ended(&mut again).await.1, RunEnd::Finished);
}

#[tokio::test]
async fn the_user_can_stop_the_run() {
    let (_server, manager, id, shown) = device(false).await;
    let runs = StepRuns::new();
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send("drip 1 2000"),
            Step::send("echo never sent"),
        ])),
    );
    events.recv().await;
    assert!(runs.stop(id));
    assert!(!runs.stop(id), "nothing left to stop");
    let (_, end) = ended(&mut events).await;
    assert!(matches!(
        end,
        RunEnd::CalledOff {
            step: 0,
            by: CalledOff::User,
            ..
        }
    ));
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert!(!shown.text().contains("never sent"));
}

#[tokio::test]
async fn the_session_ending_ends_the_run() {
    let (_server, manager, id, _shown) = device(false).await;
    let runs = StepRuns::new();
    let mut events = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send("pause 5000"),
            Step::send("echo x"),
        ])),
    );
    events.recv().await;
    manager.close(id).unwrap();
    let (_, end) = ended(&mut events).await;
    assert!(
        matches!(end, RunEnd::SessionEnded { step: 0, .. }),
        "{end:?}"
    );
    assert!(!runs.running(id));
    // A session that is not connected takes none.
    let refused = start_refused(&runs, &manager, id, plan(Steps::new(vec![Step::send("x")])));
    assert_eq!(refused.unwrap_err(), StartError::NotConnected);
}

#[tokio::test]
async fn steps_run_on_a_telnet_session() {
    let server = TestTelnetServer::start().await;
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = manager.open(SessionTarget::Telnet(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    let shown = consume(events);
    let runs = StepRuns::new();
    // The login itself, as steps: wait for each question, answer it.
    let mut run = start(
        &runs,
        &manager,
        id,
        plan(Steps::new(vec![
            Step::send(USERNAME).waiting(Wait::For("Password:".into())),
            Step::send(PASSWORD),
            Step::send("size"),
        ])),
    );
    assert_eq!(
        ended(&mut run).await.1,
        RunEnd::Finished,
        "{}",
        shown.text()
    );
    let text = shown.text();
    let prompt = String::from_utf8_lossy(TELNET_PROMPT).into_owned();
    assert!(text.ends_with(&format!("80x24\r\n{prompt}")), "{text:?}");
    assert!(text.contains(std::str::from_utf8(PASSWORD_PROMPT).unwrap()));
}

#[tokio::test]
async fn steps_run_on_a_serial_session() {
    let ports = VirtualSerialPorts::new();
    let console = ports.add("COM3", "USB Serial Port");
    let manager = SessionManager::with_serial_ports(
        known_hosts(),
        OutputOptions::default(),
        Arc::new(ports.clone()),
    );
    let (id, _shown) = connected(
        &manager,
        SessionTarget::Serial(SerialTarget {
            port: "COM3".into(),
            parameters: SerialParameters::default(),
        }),
        b"",
    )
    .await;
    let runs = StepRuns::new();
    let mut run = start(
        &runs,
        &manager,
        id,
        plan(
            Steps::new(vec![Step::send("write memory"), Step::send("show clock")])
                .answering("[confirm]", ""),
        ),
    );
    // The test is the device on the other end of the line.
    console.received_ending_with(b"write memory\r").await;
    console
        .send(b"write memory\r\nOverwrite? [confirm]")
        .unwrap();
    console.received_ending_with(b"\r").await;
    // Nothing more until the prompt is back.
    tokio::time::sleep(Duration::from_millis(500)).await;
    console.send(b"\r\n[OK]\r\nsw1#").unwrap();
    console.received_ending_with(b"show clock\r").await;
    console.send(b"show clock\r\n12:00:00 UTC\r\nsw1#").unwrap();
    let (told, end) = ended(&mut run).await;
    assert_eq!(end, RunEnd::Finished);
    assert!(
        told.contains(&RunEvent::Answered {
            index: 0,
            question: "Overwrite? [confirm]".into(),
        }),
        "{told:?}"
    );
}

#[tokio::test]
async fn a_run_starts_only_as_its_safety_allows() {
    let (_server, manager, id, shown) = device(false).await;
    let runs = StepRuns::new();
    let lab = PasteSafety {
        environment: Some(Environment::Lab),
        line_delay_ms: 0,
    };
    let production = PasteSafety {
        environment: Some(Environment::Production),
        line_delay_ms: 0,
    };
    let refused = Steps::new(vec![Step::send("echo refused")]);
    let try_start = |safety: &PasteSafety, confirm_multi_line, confirmed, in_group| {
        let plan = plan(refused.clone());
        let permission = Permission {
            safety: *safety,
            confirm_multi_line,
            previewed: plan.digest(),
            confirmed,
            in_group,
        };
        runs.start(&manager, id, plan, &permission, |_| {})
    };
    // Production asks, the preference off or not.
    assert_eq!(
        try_start(&production, false, false, false).unwrap_err(),
        StartError::NotConfirmed
    );
    // Elsewhere it asks as a paste of several lines does.
    assert_eq!(
        try_start(&lab, true, false, false).unwrap_err(),
        StartError::NotConfirmed
    );
    // A member of the multi-send group takes none, even confirmed.
    assert_eq!(
        try_start(&lab, true, true, true).unwrap_err(),
        StartError::InGroup
    );
    assert_eq!(
        try_start(&production, false, true, true).unwrap_err(),
        StartError::InGroup
    );
    assert!(!runs.running(id));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!shown.text().contains("refused"), "{:?}", shown.text());

    // Confirmed on production, it runs; so does an unconfirmed run where
    // the user turned the question off.
    for (safety, confirmed) in [(&production, true), (&lab, false)] {
        let plan = plan(Steps::new(vec![Step::send("echo allowed")]));
        let permission = Permission {
            safety: *safety,
            confirm_multi_line: false,
            previewed: plan.digest(),
            confirmed,
            in_group: false,
        };
        let (report, mut events) = mpsc::unbounded_channel();
        runs.start(&manager, id, plan, &permission, move |event| {
            let _ = report.send(event);
        })
        .unwrap();
        let (_, end) = ended(&mut events).await;
        assert_eq!(end, RunEnd::Finished);
    }
    assert!(shown.text().contains("echo allowed"), "{:?}", shown.text());
}
