//! Environment marking and paste safety (#23): what a session to a host is
//! marked as (production, staging, lab), whether a paste waits for the
//! user's confirmation, how it is split into lines, and the delay between
//! them, inherited like the other settings.

use std::fs;

use enodix_core::paste::{plan_paste, PasteSafety, MAX_PASTE_LINE_DELAY_MS};
use enodix_core::preferences::Preferences;
use enodix_core::workspace::{
    EditError, Effective, Environment, HostId, Origin, Settings, Workspace,
};
use tempfile::TempDir;

fn safety(environment: Option<Environment>) -> PasteSafety {
    PasteSafety {
        environment,
        line_delay_ms: 0,
    }
}

const NOT_MARKED: [Option<Environment>; 3] =
    [None, Some(Environment::Lab), Some(Environment::Staging)];

#[test]
fn a_multi_line_paste_waits_for_confirmation_with_its_exact_lines() {
    for environment in NOT_MARKED
        .into_iter()
        .chain([Some(Environment::Production)])
    {
        let plan = plan_paste(
            "interface Gi0/1\r\n shutdown\nend",
            &safety(environment),
            true,
        );
        assert!(plan.confirm, "{environment:?}");
        assert_eq!(plan.lines, ["interface Gi0/1\r\n", " shutdown\n", "end"]);
    }
}

#[test]
fn one_line_is_sent_at_once_even_with_its_line_break() {
    let production = safety(Some(Environment::Production));
    for text in ["show version", "show version\n", "show version\r\n"] {
        let plan = plan_paste(text, &production, true);
        assert!(!plan.confirm, "{text:?}");
        assert_eq!(plan.lines, [text]);
    }
    let empty = plan_paste("", &production, true);
    assert!(!empty.confirm);
    assert!(empty.lines.is_empty());
}

#[test]
fn blank_lines_and_lone_carriage_returns_count_as_lines() {
    let plan = plan_paste("a\r\rb\n\n", &safety(None), true);
    assert_eq!(plan.lines, ["a\r", "\r", "b\n", "\n"]);
    assert!(plan.confirm);
}

#[test]
fn the_confirmation_can_be_turned_off_except_for_production() {
    let text = "conf t\nno router bgp 65000\n";
    for environment in NOT_MARKED {
        assert!(
            !plan_paste(text, &safety(environment), false).confirm,
            "{environment:?}"
        );
    }
    // Production hosts always ask, whatever the preference says.
    assert!(plan_paste(text, &safety(Some(Environment::Production)), false).confirm);
    assert!(PasteSafety {
        environment: Some(Environment::Production),
        line_delay_ms: 0
    }
    .always_confirms());
    assert!(!safety(Some(Environment::Staging)).always_confirms());
}

#[test]
fn the_plan_carries_the_line_delay_within_its_limit() {
    let slow = PasteSafety {
        environment: None,
        line_delay_ms: 250,
    };
    assert_eq!(plan_paste("a\nb\n", &slow, true).line_delay_ms, 250);
    let too_slow = PasteSafety {
        environment: None,
        line_delay_ms: 1_000_000,
    };
    assert_eq!(
        plan_paste("a\nb\n", &too_slow, true).line_delay_ms,
        MAX_PASTE_LINE_DELAY_MS
    );
}

#[test]
fn the_confirmation_is_on_by_default_and_remembered_when_turned_off() {
    assert!(Preferences::default().confirm_multi_line_paste);
    let data = TempDir::new().unwrap();
    Preferences {
        confirm_multi_line_paste: false,
        ..Preferences::default()
    }
    .save(data.path())
    .unwrap();
    let text = fs::read_to_string(data.path().join("preferences.yaml")).unwrap();
    assert!(text.contains("confirm_multi_line_paste: false"), "{text}");
    assert!(!Preferences::load(data.path()).confirm_multi_line_paste);
    let json = serde_json::to_value(Preferences::default()).unwrap();
    assert_eq!(json["confirmMultiLinePaste"], true);
}

/// A workspace: `Prod` is production with a slow-paste folder below it,
/// one host there overriding the delay and another marked lab; `lab-r1`
/// at the root has nothing set.
fn marked_workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let write = |file: &str, text: &str| {
        let path = temp.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("Prod/_folder.yaml", "environment: production\n");
    write("Prod/Old/_folder.yaml", "paste_line_delay: 200\n");
    write("Prod/Old/sw1.yaml", "address: 10.0.0.1\n");
    write(
        "Prod/Old/sw2.yaml",
        "address: 10.0.0.2\npaste_line_delay: 0\n",
    );
    write(
        "Prod/Old/test.yaml",
        "address: 10.0.0.3\nenvironment: lab\n",
    );
    write("Prod/core-r1.yaml", "address: 10.0.0.4\n");
    write("lab-r1.yaml", "address: 10.0.0.5\n");
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[test]
fn the_paste_line_delay_inherits_like_the_other_settings() {
    let (_temp, workspace) = marked_workspace();
    let delay = |id: &str| {
        workspace
            .resolve_host(&HostId::new(id))
            .unwrap()
            .settings
            .paste_line_delay
    };
    let folder = |path: &str| Origin::Folder { path: path.into() };
    assert_eq!(
        delay("Prod/Old/sw1.yaml"),
        Effective {
            value: 200,
            origin: folder("Prod/Old")
        }
    );
    assert_eq!(
        delay("Prod/Old/sw2.yaml"),
        Effective {
            value: 0,
            origin: Origin::Host
        }
    );
    assert_eq!(
        delay("lab-r1.yaml"),
        Effective {
            value: 0,
            origin: Origin::Default
        }
    );
}

#[test]
fn each_host_has_the_environment_and_delay_its_sessions_use() {
    let (_temp, workspace) = marked_workspace();
    let all = workspace.host_paste_safety();
    let of = |id: &str| all.get(&HostId::new(id)).cloned();

    let production = |line_delay_ms| PasteSafety {
        environment: Some(Environment::Production),
        line_delay_ms,
    };
    assert_eq!(of("Prod/Old/sw1.yaml"), Some(production(200)));
    assert_eq!(of("Prod/Old/sw2.yaml"), Some(production(0)));
    assert_eq!(of("Prod/core-r1.yaml"), Some(production(0)));
    // The nearest environment wins, so a host can step out of production.
    assert_eq!(
        of("Prod/Old/test.yaml"),
        Some(PasteSafety {
            environment: Some(Environment::Lab),
            line_delay_ms: 200,
        })
    );
    assert_eq!(of("lab-r1.yaml"), Some(PasteSafety::default()));
    assert_eq!(all.len(), 5);

    // The same as resolving one host.
    let one = workspace
        .resolve_host(&HostId::new("Prod/Old/sw1.yaml"))
        .unwrap()
        .paste_safety();
    assert_eq!(Some(one), of("Prod/Old/sw1.yaml"));
}

#[test]
fn a_production_snippet_of_several_lines_waits_for_confirmation() {
    let (_temp, workspace) = marked_workspace();
    fs::write(
        workspace.root().join("_snippets.yaml"),
        "shut: \"interface {{port}}\\n shutdown\\n\"\n",
    )
    .unwrap();
    let workspace = Workspace::open(workspace.root()).unwrap();
    let id = HostId::new("Prod/core-r1.yaml");
    let mut answers = enodix_core::template::Variables::new();
    answers.set("port", "Gi0/1");
    let lines = workspace
        .expand_snippet("shut", Some(&id), &answers)
        .unwrap();
    // A snippet is sent as its lines, each ending in a line break.
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();

    let safety = workspace.resolve_host(&id).unwrap().paste_safety();
    let plan = plan_paste(&text, &safety, false);
    assert!(plan.confirm);
    assert_eq!(plan.lines, ["interface Gi0/1\n", " shutdown\n"]);
}

#[test]
fn the_paste_line_delay_is_written_and_checked() {
    let (_temp, mut workspace) = marked_workspace();
    let settings = Settings {
        paste_line_delay: Some(50),
        ..Settings::default()
    };
    workspace.write_folder_settings("", &settings).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.root().join("_folder.yaml")).unwrap(),
        "paste_line_delay: 50\n"
    );
    assert_eq!(
        workspace.host_paste_safety()[&HostId::new("lab-r1.yaml")].line_delay_ms,
        50
    );

    let mut host = workspace.host(&HostId::new("lab-r1.yaml")).unwrap().clone();
    host.settings.environment = Some(Environment::Staging);
    host.settings.paste_line_delay = Some(10);
    workspace.write_host(&host).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.root().join("lab-r1.yaml")).unwrap(),
        "address: 10.0.0.5\nenvironment: staging\npaste_line_delay: 10\n"
    );

    let too_slow = Settings {
        paste_line_delay: Some(MAX_PASTE_LINE_DELAY_MS + 1),
        ..Settings::default()
    };
    let error = workspace.write_folder_settings("", &too_slow).unwrap_err();
    assert!(matches!(error, EditError::Invalid(_)), "{error}");
    assert!(error.to_string().contains("paste line delay"), "{error}");
}

#[test]
fn a_malformed_paste_line_delay_is_reported_for_that_file() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("a.yaml"),
        "address: 1.1.1.1\npaste_line_delay: slow\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("b.yaml"),
        "address: 1.1.1.2\npaste_line_delay: 20000\n",
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    let problems: Vec<String> = workspace.problems().iter().map(|p| p.to_string()).collect();
    let message = "`paste_line_delay` must be a number of milliseconds from 0 to 10000";
    assert_eq!(
        problems,
        [format!("a.yaml: {message}"), format!("b.yaml: {message}")]
    );
}
