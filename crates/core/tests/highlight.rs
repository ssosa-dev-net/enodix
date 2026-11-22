//! Keyword highlighting: the built-in rules, the workspace's own rules in
//! its highlights file (editable from the app), turning rules off, bad
//! patterns, and how fast lines are matched.

mod support;

use std::fs;
use std::time::Instant;

use enodix_core::highlight::{HighlightRule, HighlightSource, Highlighter, Span, BUILT_IN_RULES};
use enodix_core::workspace::{EditError, Workspace, HIGHLIGHTS_FILE};
use support::workspaces::read;
use tempfile::TempDir;

/// The highlighted parts of `line` as (text, rule) pairs, in line order.
fn highlighted(highlighter: &Highlighter, line: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = line.chars().collect();
    highlighter
        .spans(line)
        .into_iter()
        .map(
            |Span {
                 start, end, rule, ..
             }| (chars[start..end].iter().collect(), rule),
        )
        .collect()
}

/// Only the highlighted texts of `line`.
fn texts(highlighter: &Highlighter, line: &str) -> Vec<String> {
    highlighted(highlighter, line)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|&(text, rule)| (text.to_string(), rule.to_string()))
        .collect()
}

/// A workspace whose highlights file holds `text`.
fn workspace_with(text: &str) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(HIGHLIGHTS_FILE), text).unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    (temp, workspace)
}

fn rule(name: &str, pattern: &str, color: &str) -> HighlightRule {
    HighlightRule {
        name: name.into(),
        pattern: pattern.into(),
        color: color.into(),
        enabled: true,
    }
}

#[test]
fn the_built_in_rules_mark_interface_states() {
    let highlighter = Highlighter::default();

    assert_eq!(
        highlighted(
            &highlighter,
            "GigabitEthernet0/1 is up, line protocol is up (connected)"
        ),
        pairs(&[
            ("GigabitEthernet0/1", "interface"),
            ("up", "state-up"),
            ("up", "state-up"),
            ("connected", "state-up"),
        ])
    );
    assert_eq!(
        highlighted(
            &highlighter,
            "Gi1/0/3   uplink   err-disabled 10   auto   auto 10/100/1000BaseTX"
        ),
        pairs(&[("Gi1/0/3", "interface"), ("err-disabled", "state-down")])
    );
    assert_eq!(
        texts(
            &highlighter,
            "Vlan20 is administratively down, line protocol is DOWN"
        ),
        ["Vlan20", "administratively down", "DOWN"]
    );
    assert_eq!(
        highlighted(&highlighter, "Te1/1/1  notconnect   1  full  10G"),
        pairs(&[("Te1/1/1", "interface"), ("notconnect", "state-down")])
    );
    assert_eq!(
        highlighted(
            &highlighter,
            "%LINK-3-UPDOWN: Interface Po10, changed state to down"
        ),
        pairs(&[("Po10", "interface"), ("down", "state-down")])
    );
    assert_eq!(
        highlighted(&highlighter, "Gi1/0/1 is blocking, port is in standby"),
        pairs(&[
            ("Gi1/0/1", "interface"),
            ("blocking", "state-warning"),
            ("standby", "state-warning"),
        ])
    );
    // Only whole words: "update", "backup", "download" are not states.
    assert!(highlighter
        .spans("update the backup and download it")
        .is_empty());
}

#[test]
fn the_built_in_rules_mark_ipv4_addresses() {
    let highlighter = Highlighter::default();

    assert_eq!(
        highlighted(&highlighter, " ip address 10.0.0.1 255.255.255.0"),
        pairs(&[("10.0.0.1", "ipv4"), ("255.255.255.0", "ipv4")])
    );
    assert_eq!(
        texts(
            &highlighter,
            "O    192.168.10.0/24 [110/2] via 172.16.0.254,"
        ),
        ["192.168.10.0/24", "172.16.0.254"]
    );
    // Not addresses: out of range, too many parts, part of a word.
    assert!(highlighter.spans("256.1.1.1").is_empty());
    assert!(highlighter.spans("1.2.3.4.5").is_empty());
    assert!(highlighter.spans("v10.0.0.1").is_empty());
    assert!(highlighter.spans("10.0.0.1/33").is_empty());
}

#[test]
fn the_built_in_rules_mark_ipv6_addresses() {
    let highlighter = Highlighter::default();

    assert_eq!(
        highlighted(&highlighter, " ipv6 address 2001:DB8:0:1::1/64"),
        pairs(&[("2001:DB8:0:1::1/64", "ipv6")])
    );
    assert_eq!(
        texts(
            &highlighter,
            "via fe80::a00:27ff:fe4e:66a1, ::1 and 2001:db8:0:0:0:0:0:2"
        ),
        ["fe80::a00:27ff:fe4e:66a1", "::1", "2001:db8:0:0:0:0:0:2"]
    );
    assert_eq!(
        texts(&highlighter, "::ffff:192.0.2.1"),
        ["::ffff:192.0.2.1"]
    );
    // Times, MAC addresses and a bare `::` are not addresses.
    assert!(highlighter.spans("uptime 12:34:56").is_empty());
    assert!(highlighter
        .spans("0011.22aa.bbcc 00:1a:2b:3c:4d:5e")
        .is_empty());
    assert!(highlighter.spans("a :: b").is_empty());
    assert!(highlighter.spans("2001:db8::1/129").is_empty());
}

#[test]
fn the_built_in_rules_mark_interface_names() {
    let highlighter = Highlighter::default();
    let names = [
        "GigabitEthernet1/0/24",
        "Gi0/1",
        "FastEthernet0/0.100",
        "TenGigabitEthernet1/1/1",
        "Te1/1/1",
        "TwentyFiveGigE1/0/1",
        "HundredGigE0/0/0/0",
        "Ethernet1/49",
        "Eth1/1",
        "Port-channel10",
        "Po10",
        "Vlan100",
        "Loopback0",
        "Lo0",
        "Tunnel5",
        "Serial0/0/0:1",
        "mgmt0",
        "ge-0/0/0",
        "xe-1/2/3.0",
        "et-0/0/1:2",
        "ae0",
        "irb.100",
        "Bundle-Ether1",
    ];
    for name in names {
        let line = format!("interface {name}");
        assert_eq!(texts(&highlighter, &line), [name], "{line}");
    }
    for line in ["Giga", "ethernet", "the port 1/0/1", "page0", "Pol10x"] {
        assert!(highlighter.spans(line).is_empty(), "{line}");
    }
}

#[test]
fn spans_count_characters_and_carry_the_rule_colour() {
    let highlighter = Highlighter::default();

    let spans = highlighter.spans("état → down");

    assert_eq!(
        spans,
        [Span {
            start: 7,
            end: 11,
            color: "red".into(),
            rule: "state-down".into(),
        }]
    );
}

#[test]
fn a_new_workspace_has_the_built_in_rules() {
    let temp = TempDir::new().unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    let names: Vec<&str> = workspace
        .highlight_rules()
        .iter()
        .map(|listed| listed.rule.name.as_str())
        .collect();
    let built_in: Vec<&str> = BUILT_IN_RULES.iter().map(|rule| rule.name).collect();
    assert_eq!(names, built_in);
    assert_eq!(
        names,
        [
            "state-down",
            "state-warning",
            "state-up",
            "ipv4",
            "ipv6",
            "interface"
        ]
    );
    assert!(workspace
        .highlight_rules()
        .iter()
        .all(|listed| listed.source == HighlightSource::BuiltIn
            && listed.error.is_none()
            && listed.rule.enabled));
    assert_eq!(
        workspace.highlighter().spans("Gi0/1 is up"),
        Highlighter::default().spans("Gi0/1 is up")
    );
}

#[test]
fn workspace_rules_come_from_the_highlights_file_before_the_built_in_ones() {
    let (_temp, workspace) = workspace_with(
        "bgp-idle:\n  pattern: '\\bIdle\\b'\n  color: '#FF8800'\nasn:\n  pattern: 'AS\\d+'\n  color: magenta\n",
    );

    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    let rules = workspace.highlight_rules();
    assert_eq!(rules[0].rule, rule("bgp-idle", r"\bIdle\b", "#ff8800"));
    assert_eq!(rules[0].source, HighlightSource::Workspace);
    assert_eq!(rules[1].rule, rule("asn", r"AS\d+", "magenta"));
    assert_eq!(rules.len(), 2 + BUILT_IN_RULES.len());

    let spans = workspace
        .highlighter()
        .spans("10.0.0.2  4 AS65001  Idle  down");
    let found: Vec<(&str, &str)> = spans
        .iter()
        .map(|span| (span.rule.as_str(), span.color.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            ("ipv4", "cyan"),
            ("asn", "magenta"),
            ("bgp-idle", "#ff8800"),
            ("state-down", "red"),
        ]
    );
}

#[test]
fn a_workspace_rule_wins_where_it_overlaps_a_built_in_one() {
    let (_temp, workspace) =
        workspace_with("uplinks:\n  pattern: 'Gi1/0/4[89]'\n  color: '#00ff00'\n");

    assert_eq!(
        highlighted(workspace.highlighter(), "Gi1/0/48 Gi1/0/47"),
        pairs(&[("Gi1/0/48", "uplinks"), ("Gi1/0/47", "interface")])
    );
}

#[test]
fn rules_can_be_turned_off_one_by_one() {
    let (_temp, workspace) = workspace_with(
        "ipv4:\n  enabled: false\nmine:\n  pattern: shutdown\n  color: yellow\n  enabled: false\n",
    );

    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    let ipv4 = workspace
        .highlight_rules()
        .iter()
        .find(|listed| listed.rule.name == "ipv4")
        .unwrap();
    assert_eq!(ipv4.source, HighlightSource::Changed);
    assert!(!ipv4.rule.enabled);
    // The rest of the built-in rule is as built in.
    assert_eq!(ipv4.rule.color, "cyan");
    let mine = workspace
        .highlight_rules()
        .iter()
        .find(|listed| listed.rule.name == "mine")
        .unwrap();
    assert!(!mine.rule.enabled);

    assert_eq!(
        texts(workspace.highlighter(), "10.0.0.1 CRC Gi0/1 down"),
        ["Gi0/1", "down"]
    );
}

#[test]
fn a_built_in_rule_can_be_given_another_pattern_or_colour() {
    let (_temp, workspace) =
        workspace_with("state-up:\n  color: '#00aa00'\nstate-down:\n  pattern: '(?i)\\bdown\\b'\n");

    let spans = workspace.highlighter().spans("up err-disabled down");
    let found: Vec<(&str, &str)> = spans
        .iter()
        .map(|span| (span.rule.as_str(), span.color.as_str()))
        .collect();
    assert_eq!(found, [("state-up", "#00aa00"), ("state-down", "red")]);
}

#[test]
fn an_invalid_pattern_is_reported_and_skipped_without_affecting_other_rules() {
    let (_temp, workspace) = workspace_with(
        "broken:\n  pattern: 'Gi(0/1'\n  color: red\nfine:\n  pattern: 'FINE'\n  color: green\nstate-up:\n  pattern: '['\n",
    );

    let problems = workspace.problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems
        .iter()
        .all(|problem| problem.file == HIGHLIGHTS_FILE));
    assert!(problems[0].message.contains("broken"), "{}", problems[0]);
    assert!(problems[1].message.contains("state-up"), "{}", problems[1]);

    // Still listed, so it can be fixed in the app, with why it is skipped.
    let broken = &workspace.highlight_rules()[0];
    assert_eq!(broken.rule.name, "broken");
    assert!(broken.error.as_deref().unwrap().contains("Gi(0/1"));

    // Every other rule still works; a broken built-in rule is skipped too.
    assert_eq!(
        texts(workspace.highlighter(), "Gi(0/1 FINE Gi0/2 up down"),
        ["FINE", "Gi0/2", "down"]
    );
}

#[test]
fn badly_defined_rules_are_reported_and_left_out() {
    let (_temp, workspace) = workspace_with(
        "no-pattern:\n  color: red\nno-colour:\n  pattern: x\nbad-colour:\n  pattern: x\n  color: '#12'\nnot-a-mapping: 3\nunknown-key:\n  pattern: x\n  color: red\n  bold: true\ngood:\n  pattern: GOOD\n  color: blue\n",
    );

    let messages: Vec<&str> = workspace
        .problems()
        .iter()
        .map(|problem| problem.message.as_str())
        .collect();
    assert_eq!(messages.len(), 5, "{messages:?}");
    let names: Vec<&str> = workspace
        .highlight_rules()
        .iter()
        .filter(|listed| listed.source == HighlightSource::Workspace)
        .map(|listed| listed.rule.name.as_str())
        .collect();
    assert_eq!(names, ["good"]);
    assert_eq!(texts(workspace.highlighter(), "GOOD"), ["GOOD"]);
}

#[test]
fn an_unreadable_highlights_file_keeps_the_built_in_rules() {
    let (_temp, workspace) = workspace_with("- just\n- a list\n");

    assert_eq!(workspace.problems().len(), 1);
    assert_eq!(workspace.highlight_rules().len(), BUILT_IN_RULES.len());
    assert_eq!(texts(workspace.highlighter(), "down"), ["down"]);
}

#[test]
fn a_rule_added_in_the_app_is_written_to_the_highlights_file() {
    let temp = TempDir::new().unwrap();
    let mut workspace = Workspace::open(temp.path()).unwrap();

    workspace
        .save_highlight_rule(None, &rule(" crc ", r"\bCRC\b", "#F00"))
        .unwrap();

    assert_eq!(
        read(temp.path(), HIGHLIGHTS_FILE),
        "crc:\n  pattern: \\bCRC\\b\n  color: '#ff0000'\n"
    );
    assert_eq!(
        workspace.highlight_rules()[0].rule,
        rule("crc", r"\bCRC\b", "#ff0000")
    );
    assert_eq!(texts(workspace.highlighter(), "12 CRC"), ["CRC"]);
}

#[test]
fn a_rule_edited_in_the_app_keeps_its_place_and_can_be_renamed() {
    let (temp, mut workspace) = workspace_with(
        "first:\n  pattern: one\n  color: red\nsecond:\n  pattern: two\n  color: blue\n",
    );

    let mut changed = rule("renamed", "uno", "green");
    changed.enabled = false;
    workspace
        .save_highlight_rule(Some("FIRST"), &changed)
        .unwrap();

    assert_eq!(
        read(temp.path(), HIGHLIGHTS_FILE),
        "renamed:\n  pattern: uno\n  color: green\n  enabled: false\nsecond:\n  pattern: two\n  color: blue\n"
    );
    assert_eq!(workspace.highlight_rules()[0].rule, changed);
}

#[test]
fn editing_a_built_in_rule_writes_only_what_differs() {
    let temp = TempDir::new().unwrap();
    let mut workspace = Workspace::open(temp.path()).unwrap();
    let mut ipv6 = workspace
        .highlight_rules()
        .iter()
        .find(|listed| listed.rule.name == "ipv6")
        .unwrap()
        .rule
        .clone();

    ipv6.enabled = false;
    workspace.save_highlight_rule(Some("ipv6"), &ipv6).unwrap();
    assert_eq!(
        read(temp.path(), HIGHLIGHTS_FILE),
        "ipv6:\n  enabled: false\n"
    );
    assert!(workspace.highlighter().spans("2001:db8::1").is_empty());

    // Back as built in: nothing left to write.
    ipv6.enabled = true;
    workspace.save_highlight_rule(Some("ipv6"), &ipv6).unwrap();
    assert!(!temp.path().join(HIGHLIGHTS_FILE).exists());
    assert_eq!(
        workspace
            .highlight_rules()
            .iter()
            .find(|listed| listed.rule.name == "ipv6")
            .unwrap()
            .source,
        HighlightSource::BuiltIn
    );
}

#[test]
fn deleting_removes_a_workspace_rule_and_resets_a_built_in_one() {
    let (temp, mut workspace) =
        workspace_with("mine:\n  pattern: x\n  color: red\nipv4:\n  color: yellow\n");

    workspace.delete_highlight_rule("MINE").unwrap();
    assert_eq!(
        read(temp.path(), HIGHLIGHTS_FILE),
        "ipv4:\n  color: yellow\n"
    );

    workspace.delete_highlight_rule("ipv4").unwrap();
    assert!(!temp.path().join(HIGHLIGHTS_FILE).exists());
    assert_eq!(workspace.highlight_rules().len(), BUILT_IN_RULES.len());

    // A built-in rule as built in cannot go; it can be turned off.
    assert!(matches!(
        workspace.delete_highlight_rule("ipv4"),
        Err(EditError::Invalid(_))
    ));
    assert!(matches!(
        workspace.delete_highlight_rule("nothing"),
        Err(EditError::NotFound(_))
    ));
}

#[test]
fn rules_that_cannot_work_are_refused_without_touching_the_file() {
    let (temp, mut workspace) = workspace_with("mine:\n  pattern: x\n  color: red\n");
    let before = read(temp.path(), HIGHLIGHTS_FILE);

    let refusals = [
        (None, rule("bad", "Gi(", "red")),
        (None, rule("", "x", "red")),
        (None, rule("empty", "  ", "red")),
        (None, rule("colour", "x", "reddish")),
        (None, rule("MINE", "y", "red")),
        (None, rule("ipv4", "y", "red")),
        (Some("mine"), rule("Interface", "y", "red")),
        (Some("ipv4"), rule("my-ipv4", "y", "red")),
        (Some("missing"), rule("missing", "y", "red")),
    ];
    for (previous, refused) in refusals {
        let result = workspace.save_highlight_rule(previous, &refused);
        assert!(result.is_err(), "{previous:?} {refused:?} was saved");
    }
    assert!(matches!(
        workspace.save_highlight_rule(None, &rule("bad", "Gi(", "red")),
        Err(EditError::Invalid(message)) if message.contains("Gi(")
    ));
    assert_eq!(read(temp.path(), HIGHLIGHTS_FILE), before);
}

#[test]
fn a_rule_with_a_broken_pattern_can_be_turned_off_in_the_app() {
    let (temp, mut workspace) = workspace_with(
        "broken:
  pattern: 'Gi('
  color: red
",
    );
    let mut broken = workspace.highlight_rules()[0].rule.clone();

    broken.enabled = false;
    workspace
        .save_highlight_rule(Some("broken"), &broken)
        .unwrap();

    assert_eq!(
        read(temp.path(), HIGHLIGHTS_FILE),
        "broken:
  pattern: Gi(
  color: red
  enabled: false
"
    );
    // Turning it back on is refused until the pattern is fixed.
    broken.enabled = true;
    assert!(matches!(
        workspace.save_highlight_rule(Some("broken"), &broken),
        Err(EditError::Invalid(_))
    ));
}

#[test]
fn a_changed_highlights_file_is_picked_up_on_refresh() {
    let (temp, mut workspace) = workspace_with("mine:\n  pattern: x\n  color: red\n");

    fs::write(
        temp.path().join(HIGHLIGHTS_FILE),
        "mine:\n  pattern: y\n  color: red\n",
    )
    .unwrap();

    assert!(workspace.refresh().unwrap());
    assert_eq!(workspace.highlight_rules()[0].rule.pattern, "y");
    assert!(!workspace.refresh().unwrap());
}

#[test]
fn lines_are_matched_quickly() {
    let highlighter = Highlighter::default();
    let sample = [
        "GigabitEthernet1/0/1 is up, line protocol is up (connected)",
        "  Hardware is Gigabit Ethernet, address is 0011.22aa.bbcc (bia 0011.22aa.bbcc)",
        "  Internet address is 10.10.20.1/24",
        "  MTU 1500 bytes, BW 1000000 Kbit/sec, DLY 10 usec,",
        "     5 minute input rate 1000 bits/sec, 2 packets/sec",
        "     0 input errors, 0 CRC, 0 frame, 0 overrun, 0 ignored",
        "  IPv6 address is 2001:db8:10:20::1/64",
        "Te1/1/2   to-core-2   err-disabled 1   full   10G  SFP-10GBase-LR",
    ];
    // Far more than a screen: the interface asks for a screen at a time.
    let lines: Vec<String> = (0..20_000)
        .map(|i| format!("{} {i}", sample[i % sample.len()]))
        .collect();

    let started = Instant::now();
    let spans: usize = lines.iter().map(|line| highlighter.spans(line).len()).sum();
    let elapsed = started.elapsed();

    assert!(spans >= 15_000, "{spans} spans");
    // Generous for debug builds; release is many times faster.
    assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?} for 20,000 lines");
    eprintln!("20,000 lines in {elapsed:?}");

    // One long line (output without line ends) full of matches.
    let long = "Gi0/1 up 10.0.0.1 ".repeat(5_000);
    let started = Instant::now();
    assert_eq!(highlighter.spans(&long).len(), 15_000);
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?} for a 90 KB line");
    eprintln!("a 90 KB line in {elapsed:?}");
}
