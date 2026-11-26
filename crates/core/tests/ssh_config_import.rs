//! OpenSSH config import (#26): the `Host` entries of an `~/.ssh/config`
//! as workspace hosts, values resolved as `ssh` resolves them, `ProxyJump`
//! as the jump host, what is not imported reported, and the no-overwrite
//! rule. Each import is checked by reading the workspace on disk after.

mod support;

use std::fs;

use enodix_core::import::ssh_config::{
    SshConfig, SshConfigError, SshConfigImport, SshConfigImportError,
};
use enodix_core::import::{ImportReport, PreviewOutcome, ReportEntry};
use enodix_core::workspace::{HostId, Workspace};
use support::workspaces::{fixture_copy, read};
use tempfile::TempDir;

/// The `edit` fixture (Access/sw1, Core/core-r1, Core/core-r2.yml, Empty)
/// as a writable copy, opened.
fn workspace() -> (TempDir, std::path::PathBuf, Workspace) {
    let (temp, root) = fixture_copy("edit");
    let workspace = Workspace::open(&root).unwrap();
    (temp, root, workspace)
}

fn import(config: &str, target: &str) -> SshConfigImport {
    SshConfigImport {
        config: SshConfig::parse(config),
        target: target.into(),
    }
}

/// `(line, name, reason)` of each entry.
fn entries(entries: &[ReportEntry]) -> Vec<(Option<usize>, String, Option<String>)> {
    entries
        .iter()
        .map(|e| (e.line, e.name.clone(), e.reason.clone()))
        .collect()
}

fn names(entries: &[ReportEntry]) -> Vec<String> {
    entries.iter().map(|e| e.name.clone()).collect()
}

fn created(report: &ImportReport) -> Vec<String> {
    report
        .created
        .iter()
        .map(|e| e.host.as_ref().unwrap().to_string())
        .collect()
}

fn id(path: &str) -> HostId {
    HostId::new(path)
}

#[test]
fn each_named_host_is_imported_with_its_address_and_port() {
    let (_temp, root, mut ws) = workspace();
    let config = "\
# Lab devices
Host edge-r1
    HostName 10.1.0.1
    Port 2222

Host edge-r2 edge-r3
  Hostname=edge.example.net
";
    let report = import(config, "Empty").run(&mut ws, &[]).unwrap();

    assert_eq!(
        created(&report),
        [
            "Empty/edge-r1.yaml",
            "Empty/edge-r2.yaml",
            "Empty/edge-r3.yaml"
        ]
    );
    assert_eq!(
        report.created.iter().map(|e| e.line).collect::<Vec<_>>(),
        [Some(2), Some(6), Some(6)]
    );
    assert!(report.skipped.is_empty() && report.failed.is_empty());
    assert_eq!(
        read(&root, "Empty/edge-r1.yaml"),
        "address: 10.1.0.1\nport: 2222\n"
    );
    let r2 = ws.host(&id("Empty/edge-r2.yaml")).unwrap();
    assert_eq!(r2.address, "edge.example.net");
    assert_eq!(r2.settings.port, None);
}

#[test]
fn a_host_without_a_hostname_is_reached_by_its_name_as_ssh_does() {
    let (_temp, _root, mut ws) = workspace();
    let report = import("Host core1.lab.example\n", "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/core1.lab.example.yaml"]);
    let host = ws.host(&id("Empty/core1.lab.example.yaml")).unwrap();
    assert_eq!(host.address, "core1.lab.example");
}

#[test]
fn values_come_from_every_matching_block_the_first_one_winning() {
    let (_temp, _root, mut ws) = workspace();
    let config = "\
Port 2200

Host r1
    HostName 10.0.0.1

Host r2
    HostName %h.lab.example
    Port 22

Host r* !r3
    Port 830
    HostName ignored.example

Host r3
";
    let report = import(config, "Empty").run(&mut ws, &[]).unwrap();
    assert_eq!(
        created(&report),
        ["Empty/r1.yaml", "Empty/r2.yaml", "Empty/r3.yaml"]
    );
    // The global Port comes first, so it wins over the blocks.
    let r1 = ws.host(&id("Empty/r1.yaml")).unwrap();
    assert_eq!(
        (r1.address.as_str(), r1.settings.port),
        ("10.0.0.1", Some(2200))
    );
    let r2 = ws.host(&id("Empty/r2.yaml")).unwrap();
    assert_eq!(r2.address, "r2.lab.example");
    // `r* !r3` passes r3 over: it keeps its own name as the address.
    let r3 = ws.host(&id("Empty/r3.yaml")).unwrap();
    assert_eq!((r3.address.as_str(), r3.settings.port), ("r3", Some(2200)));
}

#[test]
fn a_proxyjump_to_another_host_of_the_file_becomes_its_jump_host() {
    let (_temp, root, mut ws) = workspace();
    let config = "\
Host bastion
    HostName 192.0.2.10

Host core-r1
    HostName 10.0.0.1
    ProxyJump admin@bastion:2222

Host * !bastion
    ProxyJump bastion
";
    let preview = import(config, "Core").preview(&ws).unwrap();
    let jump = |name: &str| {
        preview
            .hosts
            .iter()
            .find(|host| host.name == name)
            .and_then(|host| host.host.as_ref())
            .and_then(|host| host.jump_host.clone())
    };
    assert_eq!(jump("bastion"), None);
    assert_eq!(jump("core-r1").as_deref(), Some("Core/bastion.yaml"));

    // core-r1 exists in Core: confirmed, it takes the jump host.
    let report = import(config, "Core")
        .run(&mut ws, &[id("Core/core-r1.yaml")])
        .unwrap();
    assert_eq!(created(&report), ["Core/bastion.yaml"]);
    assert_eq!(names(&report.updated), ["core-r1"]);
    let r1 = ws.host(&id("Core/core-r1.yaml")).unwrap();
    assert_eq!(r1.settings.jump_host.as_deref(), Some("Core/bastion.yaml"));
    assert_eq!(r1.address, "10.0.0.1");
    assert!(read(&root, "Core/bastion.yaml").contains("address: 192.0.2.10"));
}

#[test]
fn a_proxyjump_that_cannot_be_a_jump_host_is_reported_and_the_host_imported() {
    let (_temp, _root, mut ws) = workspace();
    let config = "\
Host a
    ProxyJump gw.example.net

Host b
    ProxyJump hop1,hop2

Host c
    ProxyJump none
";
    let report = import(config, "Empty").run(&mut ws, &[]).unwrap();
    assert_eq!(
        created(&report),
        ["Empty/a.yaml", "Empty/b.yaml", "Empty/c.yaml"]
    );
    for host in ["Empty/a.yaml", "Empty/b.yaml", "Empty/c.yaml"] {
        assert_eq!(ws.host(&id(host)).unwrap().settings.jump_host, None);
    }
    assert_eq!(
        entries(&report.skipped),
        [
            (
                Some(2),
                "a: ProxyJump gw.example.net".to_string(),
                Some(
                    "gw.example.net is not a Host of this file that is imported: \
                     set the host's Jump host by hand"
                        .to_string()
                )
            ),
            (
                Some(5),
                "b: ProxyJump hop1,hop2".to_string(),
                Some(
                    "more than one hop: give each jump host its own Jump host setting, \
                     the last hop here"
                        .to_string()
                )
            ),
        ]
    );
}

#[test]
fn users_keys_and_other_keywords_are_reported_once_and_never_written() {
    let (_temp, root, mut ws) = workspace();
    let config = "\
Include ~/.ssh/work.conf

Host r1
    User admin
    IdentityFile ~/.ssh/id_ed25519
    ServerAliveInterval 30

Host r2
    User operator
    ProxyCommand nc -X 5 -x proxy:1080 %h %p

Match host *.corp exec \"true\"
    User corp
";
    let report = import(config, "Empty").run(&mut ws, &[]).unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml", "Empty/r2.yaml"]);
    assert_eq!(
        entries(&report.skipped)
            .into_iter()
            .map(|(line, name, _)| (line, name))
            .collect::<Vec<_>>(),
        [
            (Some(1), "Include".to_string()),
            (Some(4), "User".to_string()),
            (Some(5), "IdentityFile".to_string()),
            (Some(6), "ServerAliveInterval".to_string()),
            (Some(10), "ProxyCommand".to_string()),
            (Some(12), "Match".to_string()),
        ]
    );
    assert_eq!(
        report.skipped[1].reason.as_deref(),
        Some("usernames belong in a credential profile; not imported")
    );
    for host in ["Empty/r1.yaml", "Empty/r2.yaml"] {
        let text = read(&root, host);
        assert!(
            !text.contains("admin") && !text.contains("operator"),
            "{text}"
        );
        assert!(!text.contains("id_ed25519"), "{text}");
    }
}

#[test]
fn wildcard_and_match_entries_are_not_hosts() {
    let (_temp, _root, mut ws) = workspace();
    let config = "\
Host *
    Port 22
Host *.lab 10.0.?.1 !bastion
Match all
";
    let preview = import(config, "Empty").preview(&ws).unwrap();
    assert!(preview.hosts.is_empty());
    let report = import(config, "Empty").run(&mut ws, &[]).unwrap();
    assert!(report.created.is_empty());
}

#[test]
fn bad_values_fail_their_host_and_the_others_import() {
    let (_temp, _root, mut ws) = workspace();
    let config = "\
Host good
    HostName 10.0.0.1
Host badport
    Port 70000
Host withport
    HostName 10.0.0.2:2222
Host token
    HostName %h.%d
Host good
    HostName 10.9.9.9
";
    let report = import(config, "Empty").run(&mut ws, &[]).unwrap();
    assert_eq!(created(&report), ["Empty/good.yaml"]);
    // A name given twice is one host; its first block's values win.
    assert_eq!(ws.host(&id("Empty/good.yaml")).unwrap().address, "10.0.0.1");
    assert_eq!(
        entries(&report.failed),
        [
            (
                Some(3),
                "badport".to_string(),
                Some("Port 70000 is not a port (1 to 65535)".to_string())
            ),
            (
                Some(5),
                "withport".to_string(),
                Some(
                    "the HostName has a port after it (10.0.0.2:2222): \
                     write 10.0.0.2 as the HostName and Port 2222"
                        .to_string()
                )
            ),
            (
                Some(7),
                "token".to_string(),
                Some("HostName %h.%d uses %d, which the import does not expand".to_string())
            ),
        ]
    );
}

#[test]
fn existing_hosts_are_skipped_unless_confirmed() {
    let (_temp, root, mut ws) = workspace();
    let before = read(&root, "Core/core-r1.yaml");
    let config = "Host core-r1\n    HostName 10.9.0.1\n";

    let preview = import(config, "Core").preview(&ws).unwrap();
    assert_eq!(
        preview.hosts[0].outcome,
        PreviewOutcome::Exists {
            id: id("Core/core-r1.yaml")
        }
    );
    let report = import(config, "Core").run(&mut ws, &[]).unwrap();
    assert!(report.created.is_empty());
    assert_eq!(names(&report.skipped), ["core-r1"]);
    assert_eq!(read(&root, "Core/core-r1.yaml"), before);
}

#[test]
fn a_target_folder_that_is_not_there_and_a_missing_file_are_reported() {
    let (temp, _root, ws) = workspace();
    let error = import("Host r1\n", "Nowhere").preview(&ws).unwrap_err();
    assert!(matches!(error, SshConfigImportError::NoSuchFolder(_)));

    let missing = temp.path().join("no-config");
    let error = SshConfig::read(&missing).unwrap_err();
    assert!(matches!(error, SshConfigError::Io(_)));
    assert_eq!(
        error.to_string(),
        "cannot read the SSH config file: there is no such file; check the path"
    );
    // A real file reads as its text does.
    let file = temp.path().join("config");
    fs::write(&file, "Host from-file\n").unwrap();
    let config = SshConfig::read(&file).unwrap();
    let preview = SshConfigImport {
        config,
        target: "Empty".into(),
    }
    .preview(&ws)
    .unwrap();
    assert_eq!(preview.hosts[0].name, "from-file");
}
