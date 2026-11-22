//! Platform definitions: the built-in ones, the workspace's own in its
//! platforms file (editable from the app), changed built-ins and reset,
//! other names, bad entries, and which definition a host's platform names.

mod support;

use std::fs;

use enodix_core::platform::{PlatformDefinition, PlatformSource, BUILT_IN_PLATFORMS};
use enodix_core::workspace::{EditError, HostId, Workspace, PLATFORMS_FILE};
use support::workspaces::{fixture, fixture_copy, read};
use tempfile::TempDir;

/// A workspace whose platforms file holds `text`.
fn workspace_with(text: &str) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(PLATFORMS_FILE), text).unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    (temp, workspace)
}

/// A definition with only a name and a prompt.
fn definition(name: &str, prompt: &str) -> PlatformDefinition {
    PlatformDefinition {
        name: name.into(),
        other_names: Vec::new(),
        prompt: prompt.into(),
        disable_paging: None,
        privilege_command: None,
        privilege_prompt: None,
        show_config: None,
        ignored_lines: None,
        file_folder: None,
    }
}

fn listed<'a>(workspace: &'a Workspace, name: &str) -> &'a enodix_core::platform::ListedPlatform {
    workspace
        .platforms()
        .iter()
        .find(|listed| listed.definition.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"))
}

fn built_in(name: &str) -> PlatformDefinition {
    BUILT_IN_PLATFORMS
        .iter()
        .find(|platform| platform.name == name)
        .unwrap()
        .definition()
}

fn matched(workspace: &Workspace, platform: &str) -> Option<String> {
    workspace
        .platform(platform)
        .map(|definition| definition.name.clone())
}

fn file_folder(workspace: &Workspace, id: &str) -> Option<String> {
    workspace
        .host_file_folder(&HostId::new(id))
        .map(str::to_string)
}

fn host_platform(workspace: &Workspace, id: &str) -> Option<String> {
    workspace
        .host_platform(&HostId::new(id))
        .map(|definition| definition.name.clone())
}

#[test]
fn a_new_workspace_has_the_six_built_in_platforms() {
    let temp = TempDir::new().unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    let names: Vec<&str> = workspace
        .platforms()
        .iter()
        .map(|listed| listed.definition.name.as_str())
        .collect();
    assert_eq!(names, ["srlinux", "eos", "frr", "ios", "nxos", "junos"]);
    for listed in workspace.platforms() {
        assert_eq!(listed.source, PlatformSource::BuiltIn);
        assert_eq!(listed.error, None, "{}", listed.definition.name);
        assert!(!listed.definition.prompt.is_empty());
    }
    assert!(workspace.problems().is_empty());
}

#[test]
fn the_built_ins_answer_to_the_names_other_tools_use() {
    let temp = TempDir::new().unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    for (name, expected) in [
        ("arista_eos", "eos"),
        ("EOS", "eos"),
        ("cisco_ios", "ios"),
        ("cisco_xe", "ios"),
        ("cisco_iosxe", "ios"),
        ("cisco_nxos", "nxos"),
        ("juniper_junos", "junos"),
        ("nokia_srl", "srlinux"),
        (" SRLinux ", "srlinux"),
        ("frrouting", "frr"),
    ] {
        assert_eq!(
            matched(&workspace, name).as_deref(),
            Some(expected),
            "{name}"
        );
    }
    assert_eq!(matched(&workspace, "vyos"), None);
    assert_eq!(matched(&workspace, ""), None);
}

#[test]
fn the_platforms_file_adds_definitions_and_changes_built_in_ones() {
    let workspace = Workspace::open(fixture("platforms")).unwrap();

    let names: Vec<(&str, PlatformSource)> = workspace
        .platforms()
        .iter()
        .map(|listed| (listed.definition.name.as_str(), listed.source))
        .collect();
    assert_eq!(
        names,
        [
            ("srlinux", PlatformSource::BuiltIn),
            ("eos", PlatformSource::Changed),
            ("frr", PlatformSource::BuiltIn),
            ("ios", PlatformSource::BuiltIn),
            ("nxos", PlatformSource::BuiltIn),
            ("junos", PlatformSource::Changed),
            ("mikrotik", PlatformSource::Workspace),
            ("broken", PlatformSource::Workspace),
        ]
    );

    // A changed built-in keeps what the file does not give.
    let eos = &listed(&workspace, "eos").definition;
    let original = built_in("eos");
    assert_eq!(eos.disable_paging.as_deref(), Some("terminal pager 0"));
    assert_eq!(eos.other_names, ["arista_eos", "ceos", "arista-ceos"]);
    assert_eq!(eos.prompt, original.prompt);
    assert_eq!(eos.privilege_command, original.privilege_command);
    assert_eq!(eos.show_config, original.show_config);

    // Only the prompt is required; a single other name is fine.
    assert_eq!(
        listed(&workspace, "mikrotik").definition,
        PlatformDefinition {
            other_names: vec!["routeros".into()],
            show_config: Some("/export".into()),
            ..definition("mikrotik", r"\[\S+@\S+\] >")
        }
    );
}

#[test]
fn invalid_patterns_are_listed_with_their_error_and_not_used() {
    let workspace = Workspace::open(fixture("platforms")).unwrap();

    let broken = listed(&workspace, "broken");
    assert!(broken.error.as_deref().unwrap().contains("prompt"));
    let junos = listed(&workspace, "junos");
    assert!(junos.error.as_deref().unwrap().contains("ignored_lines"));
    assert_eq!(junos.definition.ignored_lines.as_deref(), Some("[bad"));

    assert_eq!(matched(&workspace, "broken"), None);
    assert_eq!(matched(&workspace, "junos"), None);
    assert_eq!(matched(&workspace, "juniper_junos"), None);
    assert_eq!(listed(&workspace, "eos").error, None);
}

#[test]
fn badly_defined_entries_are_problems_and_left_out() {
    let workspace = Workspace::open(fixture("platforms")).unwrap();

    let messages: Vec<&str> = workspace
        .problems()
        .iter()
        .filter(|problem| problem.file == PLATFORMS_FILE)
        .map(|problem| problem.message.as_str())
        .collect();
    assert_eq!(messages.len(), 5, "{messages:#?}");
    for (message, name) in
        messages
            .iter()
            .zip(["broken", "junos", "no-prompt", "not-a-mapping", "odd-key"])
    {
        assert!(message.contains(&format!("`{name}`")), "{message}");
    }
    assert!(messages[2].contains("prompt"));
    assert!(messages[4].contains("colour"));
    for name in ["no-prompt", "not-a-mapping", "odd-key"] {
        assert!(workspace
            .platforms()
            .iter()
            .all(|listed| listed.definition.name != name));
    }
}

#[test]
fn a_hosts_platform_finds_its_definition_by_name_or_other_name_ignoring_case() {
    let workspace = Workspace::open(fixture("platforms")).unwrap();

    assert_eq!(host_platform(&workspace, "r1.yaml").as_deref(), Some("eos"));
    assert_eq!(host_platform(&workspace, "r2.yaml").as_deref(), Some("ios"));
    assert_eq!(
        host_platform(&workspace, "r3.yaml").as_deref(),
        Some("mikrotik")
    );
    assert_eq!(host_platform(&workspace, "r4.yaml"), None);
    assert_eq!(host_platform(&workspace, "b1.yaml"), None);
    // Inherited from the folder, whose definition has a broken pattern.
    assert_eq!(host_platform(&workspace, "Junos/j1.yaml"), None);
    assert_eq!(host_platform(&workspace, "missing.yaml"), None);

    // The lookup the interface uses: every usable name and other name.
    let names = workspace.platform_names();
    let found = |text: &str| {
        names
            .get(text)
            .map(|name| (name.definition.as_str(), name.usable))
    };
    assert_eq!(found("arista-ceos"), Some(("eos", true)));
    assert_eq!(found("routeros"), Some(("mikrotik", true)));
    assert_eq!(found("ios"), Some(("ios", true)));
    assert_eq!(found("broken"), Some(("broken", false)));
    assert_eq!(found("juniper_junos"), Some(("junos", false)));
    assert_eq!(found("unknown-os"), None);
}

#[test]
fn a_definition_created_in_the_app_is_written_to_the_platforms_file() {
    let temp = TempDir::new().unwrap();
    let mut workspace = Workspace::open(temp.path()).unwrap();

    let mut new = definition(" vyos ", r"\S+@\S+[$#]");
    new.other_names = vec![" VyOS-Router ".into(), "".into()];
    new.disable_paging = Some("set terminal length 0".into());
    new.show_config = Some("  ".into());
    workspace.save_platform(None, &new).unwrap();

    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "vyos:\n  other_names:\n  - VyOS-Router\n  prompt: \\S+@\\S+[$#]\n  disable_paging: set terminal length 0\n"
    );
    assert_eq!(
        listed(&workspace, "vyos").definition,
        PlatformDefinition {
            other_names: vec!["VyOS-Router".into()],
            disable_paging: Some("set terminal length 0".into()),
            ..definition("vyos", r"\S+@\S+[$#]")
        }
    );
    assert_eq!(listed(&workspace, "vyos").source, PlatformSource::Workspace);
    assert_eq!(matched(&workspace, "vyos-router").as_deref(), Some("vyos"));
}

#[test]
fn a_definition_edited_in_the_app_keeps_its_place_and_can_be_renamed() {
    let (temp, mut workspace) = workspace_with("first:\n  prompt: one\nsecond:\n  prompt: two\n");

    workspace
        .save_platform(Some("FIRST"), &definition("renamed", "uno"))
        .unwrap();

    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "renamed:\n  prompt: uno\nsecond:\n  prompt: two\n"
    );
    assert_eq!(matched(&workspace, "first"), None);
    assert_eq!(matched(&workspace, "renamed").as_deref(), Some("renamed"));
}

#[test]
fn changing_a_built_in_writes_only_the_differences_and_reset_removes_them() {
    let temp = TempDir::new().unwrap();
    let mut workspace = Workspace::open(temp.path()).unwrap();

    let mut ios = built_in("ios");
    ios.disable_paging = Some("terminal length 512".into());
    ios.privilege_command = None;
    ios.other_names.push("cisco_ios_telnet".into());
    workspace.save_platform(Some("ios"), &ios).unwrap();

    let names: Vec<String> = built_in("ios")
        .other_names
        .iter()
        .map(|name| format!("  - {name}\n"))
        .collect();
    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        format!(
            "ios:\n  other_names:\n{}  - cisco_ios_telnet\n  disable_paging: terminal length 512\n  privilege_command: ''\n",
            names.concat()
        )
    );
    let changed = listed(&workspace, "ios");
    assert_eq!(changed.source, PlatformSource::Changed);
    assert_eq!(changed.definition, ios);
    assert_eq!(
        matched(&workspace, "cisco_ios_telnet").as_deref(),
        Some("ios")
    );

    // Reset: the built-in again, and nothing left in the file.
    workspace.delete_platform("IOS").unwrap();
    assert!(!temp.path().join(PLATFORMS_FILE).exists());
    assert_eq!(listed(&workspace, "ios").source, PlatformSource::BuiltIn);
    assert_eq!(listed(&workspace, "ios").definition, built_in("ios"));

    // Saved as built in: nothing is written.
    workspace
        .save_platform(Some("ios"), &built_in("ios"))
        .unwrap();
    assert!(!temp.path().join(PLATFORMS_FILE).exists());
}

#[test]
fn deleting_removes_a_workspace_definition_but_not_a_built_in_one() {
    let (temp, mut workspace) =
        workspace_with("mine:\n  prompt: x\neos:\n  show_config: show run\n");

    workspace.delete_platform("MINE").unwrap();
    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "eos:\n  show_config: show run\n"
    );
    workspace.delete_platform("eos").unwrap();
    assert!(!temp.path().join(PLATFORMS_FILE).exists());

    assert!(matches!(
        workspace.delete_platform("eos"),
        Err(EditError::Invalid(_))
    ));
    assert!(matches!(
        workspace.delete_platform("nothing"),
        Err(EditError::NotFound(_))
    ));
}

#[test]
fn definitions_that_cannot_work_are_refused_without_touching_the_file() {
    let (temp, mut workspace) = workspace_with("mine:\n  prompt: x\n  other_names: [my-os]\n");
    let before = read(temp.path(), PLATFORMS_FILE);

    let with_names = |name: &str, other: &str| PlatformDefinition {
        other_names: vec![other.into()],
        ..definition(name, "x")
    };
    let refusals = [
        (None, definition("bad", "Gi(")),
        (None, definition("empty", "  ")),
        (None, definition("", "x")),
        (
            None,
            PlatformDefinition {
                privilege_prompt: Some("(?i)password(".into()),
                ..definition("bad-privilege", "x")
            },
        ),
        (
            None,
            PlatformDefinition {
                ignored_lines: Some("[".into()),
                ..definition("bad-ignored", "x")
            },
        ),
        (None, definition("MINE", "y")),
        (None, definition("eos", "y")),
        (None, definition("arista_eos", "y")),
        (None, definition("My-OS", "y")),
        (None, with_names("other", "IOS")),
        (None, with_names("other", "cisco_ios")),
        (None, with_names("other", "my-os")),
        (Some("mine"), definition("ios", "y")),
        (Some("eos"), definition("my-eos", "y")),
        (Some("missing"), definition("missing", "y")),
    ];
    for (previous, refused) in refusals {
        let result = workspace.save_platform(previous, &refused);
        assert!(result.is_err(), "{previous:?} {refused:?} was saved");
    }
    assert!(matches!(
        workspace.save_platform(None, &definition("bad", "Gi(")),
        Err(EditError::Invalid(message)) if message.contains("Gi(")
    ));
    assert_eq!(read(temp.path(), PLATFORMS_FILE), before);

    // A definition may keep its own other names when saved again.
    workspace
        .save_platform(Some("mine"), &with_names("mine", "MY-OS"))
        .unwrap();
}

#[test]
fn a_definition_with_an_invalid_pattern_can_be_fixed_in_the_app() {
    let (dir, root) = fixture_copy("platforms");
    let mut workspace = Workspace::open(&root).unwrap();

    let mut broken = listed(&workspace, "broken").definition.clone();
    broken.prompt = r"\S+#".into();
    workspace.save_platform(Some("broken"), &broken).unwrap();

    assert_eq!(listed(&workspace, "broken").error, None);
    assert_eq!(
        host_platform(&workspace, "b1.yaml").as_deref(),
        Some("broken")
    );
    assert!(
        read(&root, PLATFORMS_FILE).contains("broken:\n  prompt: \\S+#\n  show_config: show run\n")
    );
    drop(dir);
}

#[test]
fn a_changed_platforms_file_is_picked_up_on_refresh() {
    let (temp, mut workspace) = workspace_with("mine:\n  prompt: x\n");

    fs::write(
        temp.path().join(PLATFORMS_FILE),
        "mine:\n  prompt: y\n  other_names: theirs\n",
    )
    .unwrap();

    assert!(workspace.refresh().unwrap());
    assert_eq!(listed(&workspace, "mine").definition.prompt, "y");
    assert_eq!(matched(&workspace, "theirs").as_deref(), Some("mine"));
    assert!(!workspace.refresh().unwrap());
}

#[test]
fn other_names_that_find_another_definition_are_problems() {
    let (_temp, workspace) = workspace_with(
        "mine:\n  prompt: x\n  other_names: [eos, shared]\nyours:\n  prompt: y\n  other_names: shared\n",
    );

    let messages: Vec<&str> = workspace
        .problems()
        .iter()
        .map(|problem| problem.message.as_str())
        .collect();
    assert_eq!(messages.len(), 2, "{messages:#?}");
    assert!(messages[0].contains("`eos`") && messages[0].contains("`mine`"));
    assert!(messages[1].contains("`shared`") && messages[1].contains("`yours`"));
    // Names come first, then the first definition with the other name.
    assert_eq!(matched(&workspace, "eos").as_deref(), Some("eos"));
    assert_eq!(matched(&workspace, "shared").as_deref(), Some("mine"));
}

#[test]
fn an_unreadable_platforms_file_keeps_the_built_in_ones() {
    let (_temp, workspace) = workspace_with("- just\n- a list\n");

    assert_eq!(workspace.platforms().len(), BUILT_IN_PLATFORMS.len());
    assert!(workspace
        .problems()
        .iter()
        .any(|problem| problem.file == PLATFORMS_FILE));
    assert_eq!(matched(&workspace, "eos").as_deref(), Some("eos"));
}

#[test]
fn the_built_ins_say_where_files_belong() {
    let folders: Vec<(&str, Option<String>)> = BUILT_IN_PLATFORMS
        .iter()
        .map(|platform| (platform.name, platform.definition().file_folder))
        .collect();
    let some = |folder: &str| Some(folder.to_string());
    assert_eq!(
        folders,
        [
            ("srlinux", None),
            ("eos", some("/mnt/flash")),
            ("frr", None),
            ("ios", some("flash:")),
            ("nxos", some("bootflash:")),
            ("junos", some("/var/tmp")),
        ]
    );
}

#[test]
fn a_file_folder_is_read_and_written_with_the_other_values() {
    // Surrounding blanks of a path are dropped, whoever wrote the file.
    let (temp, mut workspace) = workspace_with("mine:\n  prompt: x\n  file_folder: ' disk0: '\n");
    assert_eq!(
        listed(&workspace, "mine").definition.file_folder.as_deref(),
        Some("disk0:")
    );
    assert!(workspace.problems().is_empty());

    let mut vyos = definition("vyos", "y");
    vyos.show_config = Some("show configuration".into());
    vyos.file_folder = Some(" /config ".into());
    workspace.save_platform(None, &vyos).unwrap();
    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "mine:\n  prompt: x\n  file_folder: ' disk0: '\nvyos:\n  prompt: y\n  show_config: show configuration\n  file_folder: /config\n"
    );
    assert_eq!(
        listed(&workspace, "vyos").definition.file_folder.as_deref(),
        Some("/config")
    );

    // Cleared: the key goes.
    vyos.file_folder = Some("  ".into());
    workspace.save_platform(Some("vyos"), &vyos).unwrap();
    assert!(read(temp.path(), PLATFORMS_FILE)
        .ends_with("vyos:\n  prompt: y\n  show_config: show configuration\n"));
    assert_eq!(listed(&workspace, "vyos").definition.file_folder, None);

    let (_bad, bad) = workspace_with("bad:\n  prompt: x\n  file_folder: [a]\n");
    assert!(matches!(
        bad.problems().first(),
        Some(problem) if problem.message.contains("file_folder")
    ));
}

#[test]
fn a_changed_built_in_can_set_change_or_clear_its_file_folder() {
    let temp = TempDir::new().unwrap();
    let mut workspace = Workspace::open(temp.path()).unwrap();

    let mut eos = built_in("eos");
    eos.file_folder = Some("/mnt/flash/images".into());
    workspace.save_platform(Some("eos"), &eos).unwrap();
    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "eos:\n  file_folder: /mnt/flash/images\n"
    );
    assert_eq!(listed(&workspace, "eos").definition, eos);

    eos.file_folder = None;
    workspace.save_platform(Some("eos"), &eos).unwrap();
    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "eos:\n  file_folder: ''\n"
    );
    assert_eq!(listed(&workspace, "eos").definition.file_folder, None);

    let mut frr = built_in("frr");
    frr.file_folder = Some("/tmp".into());
    workspace.save_platform(Some("frr"), &frr).unwrap();
    assert_eq!(
        listed(&workspace, "frr").definition.file_folder.as_deref(),
        Some("/tmp")
    );

    // Back to the built-in value: nothing left to write for eos.
    workspace
        .save_platform(Some("eos"), &built_in("eos"))
        .unwrap();
    assert_eq!(
        read(temp.path(), PLATFORMS_FILE),
        "frr:\n  file_folder: /tmp\n"
    );
}

#[test]
fn a_hosts_file_folder_follows_its_effective_platform() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join(PLATFORMS_FILE), "mine:\n  prompt: x\n").unwrap();
    fs::create_dir(root.join("Arista")).unwrap();
    fs::write(root.join("Arista/_folder.yaml"), "platform: arista_eos\n").unwrap();
    for (file, text) in [
        ("Arista/a1.yaml", "address: 192.0.2.1\n"),
        (
            "Arista/a2.yaml",
            "address: 192.0.2.2\nplatform: juniper_junos\n",
        ),
        ("c1.yaml", "address: 192.0.2.3\nplatform: cisco_xe\n"),
        ("s1.yaml", "address: 192.0.2.4\nplatform: srlinux\n"),
        ("m1.yaml", "address: 192.0.2.5\nplatform: mine\n"),
        ("u1.yaml", "address: 192.0.2.6\nplatform: unknown-os\n"),
        ("n1.yaml", "address: 192.0.2.7\n"),
    ] {
        fs::write(root.join(file), text).unwrap();
    }
    let workspace = Workspace::open(root).unwrap();

    let folder = |id: &str| file_folder(&workspace, id);
    // From the folder's platform, the host's own, and an other name.
    assert_eq!(folder("Arista/a1.yaml").as_deref(), Some("/mnt/flash"));
    assert_eq!(folder("Arista/a2.yaml").as_deref(), Some("/var/tmp"));
    assert_eq!(folder("c1.yaml").as_deref(), Some("flash:"));
    // A definition without the value, an unknown platform, none at all.
    assert_eq!(folder("s1.yaml"), None);
    assert_eq!(folder("m1.yaml"), None);
    assert_eq!(folder("u1.yaml"), None);
    assert_eq!(folder("n1.yaml"), None);
    assert_eq!(folder("missing.yaml"), None);
}
