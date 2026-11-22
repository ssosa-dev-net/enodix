//! Inherited settings: fixture workspaces whose folders set connection
//! settings, checking what each host and folder ends up with, where each
//! value came from, and what setting or clearing one writes to the files.

mod support;

use std::fs;
use std::time::Duration;

use enodix_core::credentials::{MemoryStore, Profiles};
use enodix_core::session::{SessionState, SessionTarget, TerminalSize, DEFAULT_CONNECT_TIMEOUT};
use enodix_core::workspace::{
    EditError, Effective, EffectiveSettings, Environment, FlowControl, HostId, Origin, Parity,
    Protocol, ResolvedHost, Settings, WebLink, Workspace,
};
use support::workspaces::{fixture, fixture_copy, read};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

fn inherit() -> Workspace {
    let workspace = Workspace::open(fixture("inherit")).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    workspace
}

fn resolved(workspace: &Workspace, id: &str) -> ResolvedHost {
    workspace
        .resolve_host(&HostId::new(id))
        .unwrap_or_else(|| panic!("no host {id}"))
}

fn folder(path: &str) -> Origin {
    Origin::Folder { path: path.into() }
}

fn from<T>(value: T, origin: Origin) -> Effective<T> {
    Effective { value, origin }
}

#[test]
fn single_value_settings_resolve_to_the_nearest_definition() {
    let workspace = inherit();

    let core = resolved(&workspace, "Europe/Madrid/core-r1.yaml").settings;
    // Set on the host itself.
    assert_eq!(core.port, Some(from(22, Origin::Host)));
    assert_eq!(
        core.credential_profile,
        Some(from("madrid-ops".to_string(), Origin::Host))
    );
    // Set on its folder.
    assert_eq!(
        core.jump_host,
        Some(from(
            "Europe/Madrid/bastion.yaml".to_string(),
            folder("Europe/Madrid")
        ))
    );
    assert_eq!(core.legacy_crypto, from(true, folder("Europe/Madrid")));
    // Set further up.
    assert_eq!(
        core.environment,
        Some(from(Environment::Production, folder("Europe")))
    );
    assert_eq!(core.logging, from(false, folder("")));
    // Set nowhere.
    assert_eq!(core.protocol, from(Protocol::Ssh, Origin::Default));

    // A sibling overrides other settings and inherits the rest.
    let old = resolved(&workspace, "Europe/Madrid/old-sw1.yaml").settings;
    assert_eq!(old.protocol, from(Protocol::Telnet, Origin::Host));
    assert_eq!(old.environment, Some(from(Environment::Lab, Origin::Host)));
    assert_eq!(old.port, Some(from(2222, folder("Europe"))));
    assert_eq!(
        old.credential_profile,
        Some(from("corp-admin".to_string(), folder("")))
    );
}

#[test]
fn a_host_with_nothing_set_above_it_gets_the_defaults() {
    let workspace = inherit();

    let lab = resolved(&workspace, "lab-r1.yaml");
    assert_eq!(lab.host.address, "192.0.2.1");
    let settings = lab.settings;
    assert_eq!(settings.protocol, from(Protocol::Ssh, Origin::Default));
    // No port anywhere: the protocol's own default applies when connecting.
    assert_eq!(settings.port, None);
    assert_eq!(settings.jump_host, None);
    assert_eq!(settings.environment, None);
    assert_eq!(settings.legacy_crypto, from(false, Origin::Default));
    // Only what the workspace root sets is inherited.
    assert_eq!(settings.logging, from(false, folder("")));
    assert_eq!(
        settings.credential_profile,
        Some(from("corp-admin".to_string(), folder("")))
    );

    // Logging is on unless something turns it off.
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("r1.yaml"), "address: 10.0.0.1\n").unwrap();
    let bare = Workspace::open(temp.path()).unwrap();
    assert_eq!(
        resolved(&bare, "r1.yaml").settings.logging,
        from(true, Origin::Default)
    );
}

#[test]
fn web_links_merge_by_name_and_a_nearer_link_replaces_one_of_the_same_name() {
    let workspace = inherit();

    let links = |id| resolved(&workspace, id).settings.web_links;
    let link = |name: &str, url: &str, origin| from(WebLink::new(name, url), origin);
    assert_eq!(
        links("Europe/Madrid/core-r1.yaml"),
        [
            link("Wiki", "https://wiki.example.net/{name}", folder("")),
            link("NMS", "https://nms-eu.example.net/", folder("Europe")),
            link("GUI", "https://10.1.0.1/", Origin::Host),
        ]
    );
    assert_eq!(
        links("lab-r1.yaml"),
        [
            link("Wiki", "https://wiki.example.net/{name}", folder("")),
            link("NMS", "https://nms.example.net/", folder("")),
        ]
    );
}

#[test]
fn tags_add_up_down_the_tree() {
    let workspace = inherit();

    let core = resolved(&workspace, "Europe/Madrid/core-r1.yaml");
    // The host's own tags are only its own...
    assert_eq!(core.host.tags, ["core"]);
    // ...and it also carries every tag of the folders above it, once each,
    // from the nearest folder that sets it.
    assert_eq!(
        core.settings.tags,
        [
            from("acme".to_string(), folder("")),
            from("europe".to_string(), folder("Europe/Madrid")),
            from("madrid".to_string(), folder("Europe/Madrid")),
            from("core".to_string(), Origin::Host),
        ]
    );
    assert_eq!(
        resolved(&workspace, "lab-r1.yaml").settings.tags,
        [from("acme".to_string(), folder(""))]
    );
}

#[test]
fn serial_parameters_resolve_one_by_one_from_9600_8n1() {
    let workspace = inherit();

    let old = resolved(&workspace, "Europe/Madrid/old-sw1.yaml")
        .settings
        .serial;
    assert_eq!(old.baud_rate, from(115_200, folder("Europe")));
    assert_eq!(old.parity, from(Parity::Even, folder("Europe/Madrid")));
    assert_eq!(old.data_bits, from(8, Origin::Default));
    assert_eq!(old.stop_bits, from(1, Origin::Default));
    assert_eq!(old.flow_control, from(FlowControl::None, Origin::Default));

    let core = resolved(&workspace, "Europe/Madrid/core-r1.yaml")
        .settings
        .serial;
    assert_eq!(core.baud_rate, from(9600, Origin::Host));

    let lab = resolved(&workspace, "lab-r1.yaml").settings.serial;
    assert_eq!(lab.baud_rate, from(9600, Origin::Default));
    assert_eq!(lab.parity, from(Parity::None, Origin::Default));
}

#[test]
fn a_folder_has_effective_settings_too() {
    let workspace = inherit();

    let madrid: EffectiveSettings = workspace.resolve_folder("Europe/Madrid").unwrap();
    assert_eq!(madrid.port, Some(from(2222, folder("Europe"))));
    assert_eq!(madrid.legacy_crypto, from(true, folder("Europe/Madrid")));
    assert_eq!(
        madrid
            .tags
            .iter()
            .map(|t| t.value.as_str())
            .collect::<Vec<_>>(),
        ["acme", "europe", "madrid"]
    );

    let root = workspace.resolve_folder("").unwrap();
    assert_eq!(root.port, None);
    assert_eq!(root.logging, from(false, folder("")));

    assert!(workspace.resolve_folder("Nope").is_none());
    assert!(workspace.resolve_host(&HostId::new("nope.yaml")).is_none());
}

#[test]
fn a_folder_and_a_host_read_their_own_settings() {
    let workspace = inherit();

    let europe = workspace.folder("Europe").unwrap();
    assert_eq!(europe.settings.port, Some(2222));
    assert_eq!(europe.settings.environment, Some(Environment::Production));
    assert_eq!(europe.tags, ["europe"]);
    let core = workspace
        .host(&HostId::new("Europe/Madrid/core-r1.yaml"))
        .unwrap();
    assert_eq!(core.settings.port, Some(22));
    assert_eq!(
        core.settings.web_links,
        [WebLink::new("GUI", "https://10.1.0.1/")]
    );
    assert_eq!(core.settings.serial.baud_rate, Some(9600));
    assert_eq!(core.settings.serial.parity, None);
}

#[test]
fn the_ssh_target_uses_the_resolved_port() {
    let workspace = inherit();

    let target = |id| resolved(&workspace, id).ssh_target("admin".into(), "secret".into());
    let old = target("Europe/Madrid/old-sw1.yaml");
    assert_eq!((old.host.as_str(), old.port), ("10.1.0.9", 2222));
    assert_eq!(old.username, "admin");
    assert_eq!(target("Europe/Madrid/core-r1.yaml").port, 22);
    // No port anywhere: SSH's default.
    assert_eq!(target("lab-r1.yaml").port, 22);
}

#[tokio::test]
async fn connecting_uses_a_port_set_on_the_folder() {
    let server = TestSshServer::start().await;
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("Lab")).unwrap();
    fs::write(
        temp.path().join("Lab/_folder.yaml"),
        format!("port: {}\n", server.port()),
    )
    .unwrap();
    fs::write(temp.path().join("Lab/test-r1.yaml"), "address: 127.0.0.1\n").unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    let host = resolved(&workspace, "Lab/test-r1.yaml");

    let manager = support::manager();
    let target = SessionTarget::Ssh(host.ssh_target(USERNAME.into(), PASSWORD.into()));
    let (_id, mut events) = manager.open(target, TerminalSize { cols: 80, rows: 24 });

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[test]
fn the_connect_timeout_inherits_and_reaches_the_target_and_each_jump_host() {
    let temp = TempDir::new().unwrap();
    let write = |file: &str, text: &str| {
        let path = temp.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("Far/_folder.yaml", "connect_timeout: 40\n");
    write(
        "Far/slow-r1.yaml",
        "address: 10.9.0.1\njump_host: bastion.yaml\n",
    );
    write(
        "Far/slower-r2.yaml",
        "address: 10.9.0.2\nconnect_timeout: 90\n",
    );
    write("bastion.yaml", "address: 10.0.0.1\nconnect_timeout: 5\n");
    write("near-r3.yaml", "address: 10.0.0.3\n");
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );

    let timeout = |id: &str| resolved(&workspace, id).settings.connect_timeout;
    assert_eq!(timeout("Far/slow-r1.yaml"), from(40, folder("Far")));
    assert_eq!(timeout("Far/slower-r2.yaml"), from(90, Origin::Host));
    assert_eq!(timeout("near-r3.yaml"), from(15, Origin::Default));

    let store = MemoryStore::new();
    let target = Profiles::new(&store, &workspace)
        .prompted_target(
            &HostId::new("Far/slow-r1.yaml"),
            USERNAME.into(),
            PASSWORD.into(),
        )
        .unwrap();
    assert_eq!(target.connect_timeout, Duration::from_secs(40));
    assert_eq!(target.jump_hosts[0].connect_timeout, Duration::from_secs(5));
    assert_eq!(
        resolved(&workspace, "near-r3.yaml")
            .ssh_target(USERNAME.into(), PASSWORD.into())
            .connect_timeout,
        DEFAULT_CONNECT_TIMEOUT
    );
}

#[test]
fn settings_set_on_a_folder_are_written_to_its_settings_file() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();

    let mut settings = workspace.folder("").unwrap().settings.clone();
    settings.protocol = Some(Protocol::Ssh);
    settings.port = Some(2222);
    settings.credential_profile = Some("  noc-ro ".into());
    settings.jump_host = Some("Core/core-r1.yaml".into());
    settings.legacy_crypto = Some(true);
    settings.connect_timeout = Some(30);
    settings.logging = Some(false);
    settings.environment = Some(Environment::Staging);
    settings.web_links = vec![WebLink::new("NMS", "https://nms.example.net/{name}")];
    settings.serial.baud_rate = Some(115_200);
    settings.serial.flow_control = Some(FlowControl::Hardware);
    workspace.write_folder_settings("", &settings).unwrap();

    // The unknown `owner` key is kept where it was.
    assert_eq!(
        read(&root, "_folder.yaml"),
        "\
description: Customer network
protocol: ssh
port: 2222
credential_profile: noc-ro
jump_host: Core/core-r1.yaml
legacy_crypto: true
connect_timeout: 30
logging: false
environment: staging
serial:
  baud_rate: 115200
  flow_control: hardware
web_links:
  NMS: https://nms.example.net/{name}
owner: noc
"
    );
    // Every host below sees them at once.
    let sw1 = resolved(&workspace, "Access/sw1.yaml").settings;
    assert_eq!(sw1.port, Some(from(2222, folder(""))));
    assert_eq!(
        sw1.credential_profile,
        Some(from("noc-ro".to_string(), folder("")))
    );

    // Clearing them removes their keys again.
    workspace
        .write_folder_settings("", &Settings::default())
        .unwrap();
    assert_eq!(
        read(&root, "_folder.yaml"),
        "description: Customer network\nowner: noc\n"
    );
}

#[test]
fn settings_on_a_folder_without_a_settings_file_create_it_and_clearing_removes_it() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();

    let settings = Settings {
        port: Some(830),
        ..Settings::default()
    };
    workspace
        .write_folder_settings("Access", &settings)
        .unwrap();
    assert_eq!(read(&root, "Access/_folder.yaml"), "port: 830\n");

    workspace
        .write_folder_settings("Access", &Settings::default())
        .unwrap();
    assert!(!root.join("Access/_folder.yaml").exists());
}

#[test]
fn folder_tags_are_written_to_its_settings_file() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();

    workspace
        .set_folder_tags(
            "Core",
            &["dc1".into(), " ".into(), "dc1".into(), "spine".into()],
        )
        .unwrap();
    assert_eq!(
        read(&root, "Core/_folder.yaml"),
        "description: Core layer\ntags:\n- dc1\n- spine\n"
    );
    assert_eq!(workspace.folder("Core").unwrap().tags, ["dc1", "spine"]);
    let tags: Vec<_> = resolved(&workspace, "Core/core-r1.yaml")
        .settings
        .tags
        .into_iter()
        .map(|tag| (tag.value, tag.origin))
        .collect();
    assert_eq!(
        tags,
        [
            ("dc1".to_string(), Origin::Host),
            ("spine".to_string(), folder("Core")),
            ("core".to_string(), Origin::Host),
        ]
    );

    workspace.set_folder_tags("Core", &[]).unwrap();
    assert_eq!(
        read(&root, "Core/_folder.yaml"),
        "description: Core layer\n"
    );
}

#[test]
fn settings_set_on_a_host_are_written_to_its_file() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();
    let mut sw = workspace
        .host(&HostId::new("Access/sw1.yaml"))
        .unwrap()
        .clone();

    sw.settings.protocol = Some(Protocol::Telnet);
    sw.settings.port = Some(23);
    sw.settings.environment = Some(Environment::Lab);
    sw.settings.web_links = vec![
        WebLink::new("GUI", "https://{address}/"),
        WebLink::new("Grafana", "https://grafana/d/{name}"),
    ];
    sw.settings.serial.data_bits = Some(7);
    sw.settings.serial.parity = Some(Parity::Odd);
    sw.settings.serial.stop_bits = Some(2);
    workspace.write_host(&sw).unwrap();

    // Settings go after the address, links after the tags.
    assert_eq!(
        read(&root, "Access/sw1.yaml"),
        "\
address: 10.0.2.1
protocol: telnet
port: 23
environment: lab
serial:
  data_bits: 7
  parity: odd
  stop_bits: 2
web_links:
  GUI: https://{address}/
  Grafana: https://grafana/d/{name}
"
    );
    let reopened = Workspace::open(&root).unwrap();
    assert_eq!(
        reopened.host(&sw.id).unwrap(),
        workspace.host(&sw.id).unwrap()
    );

    sw.settings = Settings::default();
    workspace.write_host(&sw).unwrap();
    assert_eq!(read(&root, "Access/sw1.yaml"), "address: 10.0.2.1\n");
}

#[test]
fn a_serial_parameter_set_alone_keeps_the_others_in_the_file() {
    let (_temp, root) = fixture_copy("edit");
    fs::write(
        root.join("Access/_folder.yaml"),
        "serial:\n  baud_rate: 19200\n  break_length: 250\n",
    )
    .unwrap();
    let mut workspace = Workspace::open(&root).unwrap();

    let mut settings = workspace.folder("Access").unwrap().settings.clone();
    assert_eq!(settings.serial.baud_rate, Some(19200));
    settings.serial.stop_bits = Some(2);
    workspace
        .write_folder_settings("Access", &settings)
        .unwrap();

    assert_eq!(
        read(&root, "Access/_folder.yaml"),
        "serial:\n  baud_rate: 19200\n  break_length: 250\n  stop_bits: 2\n"
    );
}

#[test]
fn the_serial_port_is_saved_on_a_host_and_inherits_like_the_other_parameters() {
    let (_temp, root) = fixture_copy("edit");
    fs::write(root.join("Access/_folder.yaml"), "serial:\n  port: COM3\n").unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    let id = HostId::new("Access/sw1.yaml");

    let inherited = workspace.resolve_host(&id).unwrap().settings.serial;
    assert_eq!(
        inherited.port,
        Some(from("COM3".to_string(), folder("Access")))
    );
    assert_eq!(
        workspace.resolve_folder("").unwrap().serial.port,
        None,
        "nothing sets a port at the root"
    );

    let mut sw = workspace.host(&id).unwrap().clone();
    sw.settings.serial.port = Some("  COM12 ".into());
    sw.settings.serial.baud_rate = Some(115_200);
    workspace.write_host(&sw).unwrap();
    assert_eq!(
        read(&root, "Access/sw1.yaml"),
        "address: 10.0.2.1\nserial:\n  port: COM12\n  baud_rate: 115200\n"
    );
    let own = workspace.resolve_host(&id).unwrap().settings.serial;
    assert_eq!(own.port, Some(from("COM12".to_string(), Origin::Host)));

    // A blank port is no port: the folder's applies again.
    let mut sw = workspace.host(&id).unwrap().clone();
    sw.settings.serial.port = Some(" ".into());
    workspace.write_host(&sw).unwrap();
    assert_eq!(
        read(&root, "Access/sw1.yaml"),
        "address: 10.0.2.1\nserial:\n  baud_rate: 115200\n"
    );
    assert_eq!(
        workspace.resolve_host(&id).unwrap().settings.serial.port,
        Some(from("COM3".to_string(), folder("Access")))
    );
}

#[test]
fn invalid_settings_are_refused_and_the_file_is_untouched() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();
    let before = read(&root, "_folder.yaml");

    type Change = fn(&mut Settings);
    let refused = |change: Change| {
        let mut settings = Settings::default();
        change(&mut settings);
        settings
    };
    let cases: [(Change, &str); 9] = [
        (|s| s.port = Some(0), "port"),
        (|s| s.connect_timeout = Some(0), "connect timeout"),
        (|s| s.connect_timeout = Some(3601), "connect timeout"),
        (|s| s.serial.baud_rate = Some(0), "baud"),
        (|s| s.serial.data_bits = Some(9), "data bits"),
        (|s| s.serial.stop_bits = Some(3), "stop bits"),
        (
            |s| s.web_links = vec![WebLink::new(" ", "https://x/")],
            "name",
        ),
        (|s| s.web_links = vec![WebLink::new("GUI", " ")], "GUI"),
        (
            |s| {
                s.web_links = vec![
                    WebLink::new("GUI", "https://a/"),
                    WebLink::new("GUI", "https://b/"),
                ]
            },
            "twice",
        ),
    ];
    for (change, words) in cases {
        let error = workspace
            .write_folder_settings("", &refused(change))
            .unwrap_err();
        assert!(matches!(error, EditError::Invalid(_)), "{error}");
        assert!(error.to_string().contains(words), "{error}");
        assert_eq!(read(&root, "_folder.yaml"), before);
    }

    let error = workspace
        .write_folder_settings("Nope", &Settings::default())
        .unwrap_err();
    assert!(matches!(error, EditError::NotFound(_)), "{error}");
}

#[test]
fn malformed_settings_are_reported_for_that_file() {
    let temp = TempDir::new().unwrap();
    let write = |file: &str, text: &str| {
        let path = temp.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("a.yaml", "address: 1.1.1.1\nprotocol: ftp\n");
    write("b.yaml", "address: 1.1.1.2\nlegacy_crypto: maybe\n");
    write("c.yaml", "address: 1.1.1.3\nenvironment: prod\n");
    write("d.yaml", "address: 1.1.1.4\nweb_links: [https://x/]\n");
    write("e.yaml", "address: 1.1.1.5\nserial:\n  parity: sometimes\n");
    write("f.yaml", "address: 1.1.1.6\nserial:\n  data_bits: 4\n");
    write("g.yaml", "address: 1.1.1.8\nconnect_timeout: 0\n");
    write(
        "h.yaml",
        "address: 1.1.1.9\nserial:\n  port: [COM1, COM2]\n",
    );
    write("Site/_folder.yaml", "port: twenty-two\n");
    write("Site/r1.yaml", "address: 1.1.1.7\n");

    let workspace = Workspace::open(temp.path()).unwrap();

    let problems: Vec<String> = workspace.problems().iter().map(|p| p.to_string()).collect();
    assert_eq!(
        problems,
        [
            "Site/_folder.yaml: `port` must be a number from 1 to 65535",
            "a.yaml: `protocol` must be one of ssh, telnet, serial",
            "b.yaml: `legacy_crypto` must be true or false",
            "c.yaml: `environment` must be one of production, staging, lab",
            "d.yaml: `web_links` must map link names to URLs",
            "e.yaml: `serial.parity` must be one of none, odd, even, mark, space",
            "f.yaml: `serial.data_bits` must be a number from 5 to 8",
            "g.yaml: `connect_timeout` must be a number of seconds from 1 to 3600",
            "h.yaml: `serial.port` must be the name of a serial port, such as COM3",
        ]
    );
    // The host under the bad folder file still loads, without its settings.
    assert_eq!(resolved(&workspace, "Site/r1.yaml").settings.port, None);
}

// --- Platform (#40) ---

#[test]
fn the_platform_resolves_to_the_nearest_definition_with_its_origin() {
    let workspace = inherit();

    // The host's own value wins over its folders'.
    let core = resolved(&workspace, "Europe/Madrid/core-r1.yaml");
    assert_eq!(
        core.settings.platform,
        Some(from("srlinux".to_string(), Origin::Host))
    );
    assert_eq!(core.host.settings.platform.as_deref(), Some("srlinux"));
    // A host without one gets the nearest folder's.
    let old = resolved(&workspace, "Europe/Madrid/old-sw1.yaml");
    assert_eq!(
        old.settings.platform,
        Some(from("eos".to_string(), folder("Europe")))
    );
    assert_eq!(old.host.settings.platform, None);
    // Nothing above sets one: none applies.
    assert_eq!(resolved(&workspace, "lab-r1.yaml").settings.platform, None);

    // Folders resolve it too, and read their own.
    assert_eq!(
        workspace.resolve_folder("Europe/Madrid").unwrap().platform,
        Some(from("eos".to_string(), folder("Europe")))
    );
    assert_eq!(
        workspace
            .folder("Europe")
            .unwrap()
            .settings
            .platform
            .as_deref(),
        Some("eos")
    );
}

#[test]
fn the_platform_is_set_and_cleared_on_a_folder_and_on_a_host() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();

    let mut settings = workspace.folder("Core").unwrap().settings.clone();
    settings.platform = Some(" eos ".into());
    workspace.write_folder_settings("Core", &settings).unwrap();
    assert_eq!(
        read(&root, "Core/_folder.yaml"),
        "description: Core layer\nplatform: eos\n"
    );
    let platform =
        |workspace: &Workspace| resolved(workspace, "Core/core-r1.yaml").settings.platform;
    assert_eq!(
        platform(&workspace),
        Some(from("eos".to_string(), folder("Core")))
    );

    // The host's own value goes in its file, after the vendor, and wins.
    let mut core = workspace
        .host(&HostId::new("Core/core-r1.yaml"))
        .unwrap()
        .clone();
    core.settings.platform = Some("srlinux".into());
    workspace.write_host(&core).unwrap();
    assert!(read(&root, "Core/core-r1.yaml")
        .contains("vendor: Arista\nplatform: srlinux\nrole: core\n"));
    assert_eq!(
        platform(&workspace),
        Some(from("srlinux".to_string(), Origin::Host))
    );

    // Clearing the host's own brings back the folder's; clearing that, none.
    core.settings.platform = Some("  ".into());
    workspace.write_host(&core).unwrap();
    assert!(!read(&root, "Core/core-r1.yaml").contains("platform"));
    assert_eq!(
        platform(&workspace),
        Some(from("eos".to_string(), folder("Core")))
    );
    settings.platform = None;
    workspace.write_folder_settings("Core", &settings).unwrap();
    assert_eq!(
        read(&root, "Core/_folder.yaml"),
        "description: Core layer\n"
    );
    assert_eq!(platform(&workspace), None);
}

#[test]
fn platforms_written_as_before_still_load_and_are_not_rewritten() {
    let temp = TempDir::new().unwrap();
    let write = |file: &str, text: &str| {
        let path = temp.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    // A number, a blank value, and one with a comment.
    write("a.yaml", "address: 10.0.0.1\nplatform: 7\n");
    write("b.yaml", "address: 10.0.0.2\nplatform: ''\n");
    write(
        "c.yaml",
        "# lab box\naddress: 10.0.0.3\nplatform: ios   # old\n",
    );
    write("Site/_folder.yaml", "platform: ''\n");
    write("Site/r1.yaml", "address: 10.0.0.4\n");
    let mut workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );

    let own = |workspace: &Workspace, id: &str| {
        workspace
            .host(&HostId::new(id))
            .unwrap()
            .settings
            .platform
            .clone()
    };
    assert_eq!(own(&workspace, "a.yaml").as_deref(), Some("7"));
    assert_eq!(own(&workspace, "b.yaml"), None);
    assert_eq!(own(&workspace, "c.yaml").as_deref(), Some("ios"));
    assert_eq!(resolved(&workspace, "Site/r1.yaml").settings.platform, None);

    // Saving them unchanged writes nothing.
    for id in ["a.yaml", "c.yaml"] {
        let host = workspace.host(&HostId::new(id)).unwrap().clone();
        let before = read(temp.path(), id);
        workspace.write_host(&host).unwrap();
        assert_eq!(read(temp.path(), id), before);
    }
}

#[test]
fn backup_on_login_is_off_by_default_and_inherits_with_its_origin() {
    let (_temp, root) = fixture_copy("edit");
    let mut workspace = Workspace::open(&root).unwrap();
    let backup = |workspace: &Workspace| {
        resolved(workspace, "Core/core-r1.yaml")
            .settings
            .backup_on_login
    };
    assert_eq!(backup(&workspace), from(false, Origin::Default));

    let mut settings = workspace.folder("Core").unwrap().settings.clone();
    settings.backup_on_login = Some(true);
    workspace.write_folder_settings("Core", &settings).unwrap();
    assert_eq!(
        read(&root, "Core/_folder.yaml"),
        "description: Core layer\nbackup_on_login: true\n"
    );
    assert_eq!(backup(&workspace), from(true, folder("Core")));
    assert_eq!(
        workspace.resolve_folder("Core").unwrap().backup_on_login,
        from(true, folder("Core"))
    );

    // A host turns it off for itself (after `port`, its nearest predecessor).
    let mut core = workspace
        .host(&HostId::new("Core/core-r1.yaml"))
        .unwrap()
        .clone();
    core.settings.backup_on_login = Some(false);
    workspace.write_host(&core).unwrap();
    assert!(read(&root, "Core/core-r1.yaml").ends_with("port: 22\nbackup_on_login: false\n"));
    assert_eq!(backup(&workspace), from(false, Origin::Host));
}
