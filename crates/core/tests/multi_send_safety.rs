//! Multi-send safety (#36): which sessions must be confirmed before they
//! join the multi-send group, and how one paste is planned for every member
//! of the group: one confirmation, each member its own line delay.

use enodix_core::paste::{confirm_joining, plan_group_paste, PasteSafety, MAX_PASTE_LINE_DELAY_MS};
use enodix_core::workspace::Environment;

fn safety(environment: Option<Environment>, line_delay_ms: u32) -> PasteSafety {
    PasteSafety {
        environment,
        line_delay_ms,
    }
}

const NONE: PasteSafety = PasteSafety {
    environment: None,
    line_delay_ms: 0,
};

fn production() -> PasteSafety {
    safety(Some(Environment::Production), 0)
}

// --- Joining ---

#[test]
fn only_production_sessions_are_confirmed_before_joining() {
    let joining = [
        ("lab", safety(Some(Environment::Lab), 0)),
        ("core", production()),
        ("quick", NONE),
        ("staging", safety(Some(Environment::Staging), 0)),
        ("edge", safety(Some(Environment::Production), 100)),
    ];
    assert_eq!(confirm_joining(&joining), ["core", "edge"]);
    assert!(confirm_joining(&joining[..1]).is_empty());
    assert!(confirm_joining::<u64>(&[]).is_empty());
}

#[test]
fn a_production_session_asks_to_join_even_with_paste_confirmation_off() {
    // Joining asks whatever the paste preference: it is not a paste.
    assert!(production().confirms_joining());
    assert!(!NONE.confirms_joining());
}

// --- One paste for the group ---

#[test]
fn a_multi_line_paste_to_the_group_asks_once_if_any_member_would_ask() {
    let text = "conf t\nhostname x\nend\n";
    // With the preference on, every member would ask.
    let plan = plan_group_paste(&[(text, NONE), (text, NONE)], true);
    assert!(plan.confirm);

    // With it off, only a production member asks, and then the group does.
    let plan = plan_group_paste(&[(text, NONE), (text, production()), (text, NONE)], false);
    assert!(plan.confirm);
    assert_eq!(plan.environment, Some(Environment::Production));
}

#[test]
fn with_the_preference_off_and_no_production_member_the_paste_is_not_confirmed() {
    let text = "a\nb\n";
    let plan = plan_group_paste(
        &[
            (text, safety(Some(Environment::Lab), 0)),
            (text, safety(Some(Environment::Staging), 0)),
            (text, NONE),
        ],
        false,
    );
    assert!(!plan.confirm);
    // The strictest environment is still shown.
    assert_eq!(plan.environment, Some(Environment::Staging));
}

#[test]
fn a_single_line_never_asks_even_with_production_members() {
    for text in ["show version", "show version\r\n"] {
        let plan = plan_group_paste(&[(text, production()), (text, production())], true);
        assert!(!plan.confirm, "{text:?}");
        assert!(plan.members.iter().all(|member| member.lines.len() == 1));
    }
}

#[test]
fn each_member_keeps_its_own_lines_and_line_delay() {
    let plan = plan_group_paste(
        &[
            ("a\nb\n", safety(None, 0)),
            ("a\r\nb", safety(Some(Environment::Lab), 250)),
            ("a\nb\n", safety(Some(Environment::Production), 1_000_000)),
        ],
        true,
    );
    let delays: Vec<u32> = plan.members.iter().map(|m| m.line_delay_ms).collect();
    assert_eq!(delays, [0, 250, MAX_PASTE_LINE_DELAY_MS]);
    assert_eq!(plan.members[0].lines, ["a\n", "b\n"]);
    assert_eq!(plan.members[1].lines, ["a\r\n", "b"]);
}

#[test]
fn members_filled_differently_ask_if_any_of_them_has_several_lines() {
    // A snippet value holding a line break gives one member more lines.
    let plan = plan_group_paste(&[("one\n", NONE), ("one\ntwo\n", NONE)], true);
    assert!(plan.confirm);
}

#[test]
fn the_strictest_environment_is_production_then_staging_then_lab() {
    let text = "a\nb\n";
    let environment = |members: &[PasteSafety]| {
        let members: Vec<_> = members.iter().map(|&safety| (text, safety)).collect();
        plan_group_paste(&members, true).environment
    };
    let lab = safety(Some(Environment::Lab), 0);
    let staging = safety(Some(Environment::Staging), 0);
    assert_eq!(environment(&[NONE, NONE]), None);
    assert_eq!(environment(&[NONE, lab]), Some(Environment::Lab));
    assert_eq!(
        environment(&[lab, staging, NONE]),
        Some(Environment::Staging)
    );
    assert_eq!(
        environment(&[staging, production(), lab]),
        Some(Environment::Production)
    );
    assert_eq!(environment(&[]), None);
}
