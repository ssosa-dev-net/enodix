//! The layouts of the app's windows and the one multi-send group that spans
//! them (#83): each window's tabs and panes are its own, a session window
//! shows no view tabs, and the group routes input by session whichever
//! window a member is in.

use netterm_core::layout::{
    LayoutError, Member, PaneId, Split, TabId, Taken, Used, View, WindowId, Windows,
};
use netterm_core::session::SessionId;
use serde_json::json;

const MAIN: WindowId = WindowId::MAIN;

/// Opens `count` session tabs in the active pane of `window`.
fn open(windows: &mut Windows, window: WindowId, count: usize) -> Vec<TabId> {
    let mut layout = windows.layout_mut(window).expect("the window exists");
    (0..count).map(|_| layout.open_tab()).collect()
}

fn member(window: WindowId, tab: TabId) -> Member {
    Member { window, tab }
}

/// The members in the main window, in the order they joined.
fn main_group(windows: &Windows) -> Vec<TabId> {
    windows.window_group(MAIN)
}

fn revision(windows: &Windows, window: WindowId) -> u64 {
    windows
        .layout(window)
        .expect("the window exists")
        .revision()
}

fn preferences() -> View {
    View::new("preferences", None)
}

// The windows and their layouts.

#[test]
fn the_app_starts_with_the_main_windows_empty_layout() {
    let windows = Windows::new();

    let layout = windows.layout(MAIN).expect("the main window has a layout");
    assert_eq!(layout.panes().len(), 1);
    assert_eq!(layout.active_tab(), None);
    assert_eq!(windows.windows(), vec![MAIN]);
}

#[test]
fn each_windows_layout_is_its_own() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };

    let main = windows.layout(MAIN).unwrap();
    let other = windows.layout(session_window).unwrap();
    assert_eq!(main.pane(main.active_pane()).unwrap().tabs, vec![a, b]);
    assert_eq!(other.active_tab(), None);
    assert_eq!(other.pane_of(a), None);
    assert_ne!(session_window, MAIN);
    assert_eq!(windows.windows(), vec![MAIN, session_window]);

    let c = open(&mut windows, session_window, 1)[0];
    let main = windows.layout(MAIN).unwrap();
    assert_eq!(main.pane(main.active_pane()).unwrap().tabs, vec![a, b]);
    assert!(windows.layout(session_window).unwrap().pane_of(c).is_some());
}

#[test]
fn a_session_windows_layout_refuses_view_tabs() {
    let mut windows = Windows::new();
    let window = windows.open_session_window();
    let before = revision(&windows, window);

    let mut layout = windows.layout_mut(window).unwrap();
    assert_eq!(
        layout.open_view(preferences()),
        Err(LayoutError::NoViewTabs)
    );
    drop(layout);

    assert_eq!(revision(&windows, window), before);
    assert_eq!(windows.layout(window).unwrap().active_tab(), None);
    // The main window's layout still opens them.
    assert!(windows
        .layout_mut(MAIN)
        .unwrap()
        .open_view(preferences())
        .is_ok());
}

#[test]
fn an_unknown_window_has_no_layout() {
    let mut windows = Windows::new();
    let gone = WindowId(99);

    assert!(windows.layout(gone).is_none());
    assert!(windows.layout_mut(gone).is_none());
    assert_eq!(windows.reset(gone), Err(LayoutError::NoSuchWindow(gone)));
    assert_eq!(
        windows.join_group(gone, TabId(1)),
        Err(LayoutError::NoSuchWindow(gone))
    );
}

#[test]
fn resetting_a_window_starts_its_layout_afresh_and_drops_its_members() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    let b = open(&mut windows, session_window, 1)[0];
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(session_window, b).unwrap();

    windows.reset(MAIN).unwrap();

    assert_eq!(windows.layout(MAIN).unwrap().active_tab(), None);
    assert_eq!(windows.group(), &[member(session_window, b)]);
    // A session window stays one.
    windows.reset(session_window).unwrap();
    assert_eq!(
        windows
            .layout_mut(session_window)
            .unwrap()
            .open_view(preferences()),
        Err(LayoutError::NoViewTabs)
    );
}

#[test]
fn closing_a_session_window_drops_its_layout_and_its_members() {
    let mut windows = Windows::new();
    let window = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    let b = open(&mut windows, window, 1)[0];
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(window, b).unwrap();

    windows.close_window(window).unwrap();

    assert!(windows.layout(window).is_none());
    assert_eq!(windows.group(), &[member(MAIN, a)]);
    assert_eq!(windows.close_window(MAIN), Err(LayoutError::MainWindow));
}

#[test]
fn the_interface_sees_its_windows_layout_with_that_windows_members() {
    let mut windows = Windows::new();
    let other = windows.open_session_window();
    let left = windows.layout(MAIN).unwrap().active_pane();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    let right = windows
        .layout_mut(MAIN)
        .unwrap()
        .split(left, Split::Down)
        .unwrap();
    let elsewhere = open(&mut windows, other, 1)[0];
    windows.join_group(other, elsewhere).unwrap();
    windows.join_group(MAIN, b).unwrap();

    assert_eq!(
        serde_json::to_value(windows.view(MAIN).unwrap()).unwrap(),
        json!({
            "root": {
                "kind": "split",
                "direction": "down",
                "children": [
                    { "kind": "pane", "id": left.0, "tabs": [a.0], "active": a.0 },
                    { "kind": "pane", "id": right.0, "tabs": [b.0], "active": b.0 },
                ],
            },
            "activePane": right.0,
            "group": [b.0],
            "groupSize": 2,
            "kinds": {
                a.0.to_string(): { "kind": "session" },
                b.0.to_string(): { "kind": "session" },
            },
            "revision": revision(&windows, MAIN),
        })
    );
}

// The multi-send group (#35), one for the app.

#[test]
fn tabs_join_and_leave_the_multi_send_group() {
    let mut windows = Windows::new();
    let [a, b, c] = open(&mut windows, MAIN, 3)[..] else {
        unreachable!()
    };

    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, c).unwrap();
    assert_eq!(main_group(&windows), vec![a, c]);
    assert!(windows.in_group(MAIN, a));
    assert!(!windows.in_group(MAIN, b));

    windows.leave_group(MAIN, a).unwrap();
    assert_eq!(main_group(&windows), vec![c]);
}

#[test]
fn joining_twice_or_leaving_when_not_a_member_changes_nothing() {
    let mut windows = Windows::new();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.join_group(MAIN, a).unwrap();
    let before = revision(&windows, MAIN);

    windows.join_group(MAIN, a).unwrap();
    windows.leave_group(MAIN, b).unwrap();

    assert_eq!(main_group(&windows), vec![a]);
    assert_eq!(revision(&windows, MAIN), before);
}

#[test]
fn ending_the_group_removes_every_member() {
    let mut windows = Windows::new();
    let other = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    let b = open(&mut windows, other, 1)[0];
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(other, b).unwrap();
    let before = revision(&windows, MAIN);

    windows.end_group();

    assert_eq!(windows.group(), &[] as &[Member]);
    assert!(revision(&windows, MAIN) > before);
}

#[test]
fn group_edits_raise_every_windows_revision_and_refused_ones_change_nothing() {
    let mut windows = Windows::new();
    let other = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    let (mut main, mut elsewhere) = (revision(&windows, MAIN), revision(&windows, other));

    windows.join_group(MAIN, a).unwrap();
    assert!(revision(&windows, MAIN) > main);
    assert!(revision(&windows, other) > elsewhere);
    (main, elsewhere) = (revision(&windows, MAIN), revision(&windows, other));
    windows.leave_group(MAIN, a).unwrap();
    assert!(revision(&windows, MAIN) > main);
    assert!(revision(&windows, other) > elsewhere);
    main = revision(&windows, MAIN);

    let gone = TabId(999);
    assert_eq!(
        windows.join_group(MAIN, gone),
        Err(LayoutError::NoSuchTab(gone))
    );
    assert_eq!(
        windows.leave_group(MAIN, gone),
        Err(LayoutError::NoSuchTab(gone))
    );
    assert_eq!(
        windows.attach_session(MAIN, gone, SessionId(1)),
        Err(LayoutError::NoSuchTab(gone))
    );
    // A tab of the main window is not a tab of another window.
    assert_eq!(windows.join_group(other, a), Err(LayoutError::NoSuchTab(a)));
    assert_eq!(revision(&windows, MAIN), main);
    assert_eq!(windows.group(), &[] as &[Member]);
}

#[test]
fn closing_or_dismissing_a_member_tab_removes_it_from_the_group() {
    let mut windows = Windows::new();
    let [a, b, c] = open(&mut windows, MAIN, 3)[..] else {
        unreachable!()
    };
    for tab in [a, b, c] {
        windows.join_group(MAIN, tab).unwrap();
    }

    windows.layout_mut(MAIN).unwrap().close_tab(a).unwrap();
    windows.layout_mut(MAIN).unwrap().dismiss_tab(b).unwrap();

    assert_eq!(main_group(&windows), vec![c]);
}

#[test]
fn closing_a_member_tab_raises_every_windows_revision() {
    let mut windows = Windows::new();
    let other = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.join_group(MAIN, a).unwrap();
    let elsewhere = revision(&windows, other);

    windows.layout_mut(MAIN).unwrap().close_tab(b).unwrap();
    assert_eq!(revision(&windows, other), elsewhere, "b was not a member");

    windows.layout_mut(MAIN).unwrap().close_tab(a).unwrap();
    assert!(revision(&windows, other) > elsewhere);
}

#[test]
fn moving_a_member_tab_keeps_it_in_the_group() {
    let mut windows = Windows::new();
    let left = windows.layout(MAIN).unwrap().active_pane();
    let [a, b, c] = open(&mut windows, MAIN, 3)[..] else {
        unreachable!()
    };
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, c).unwrap();

    let mut layout = windows.layout_mut(MAIN).unwrap();
    layout.move_tab_by(a, 1).unwrap();
    let right = layout.split(left, Split::Right).unwrap();
    layout.move_tab(a, right, 0).unwrap();
    layout.close_pane(right).unwrap();
    drop(layout);

    assert_eq!(main_group(&windows), vec![a, c]);
    assert!(windows.layout(MAIN).unwrap().pane_of(b).is_some());
}

#[test]
fn new_and_duplicated_tabs_start_outside_the_group() {
    let mut windows = Windows::new();
    let a = open(&mut windows, MAIN, 1)[0];
    windows.join_group(MAIN, a).unwrap();

    let mut layout = windows.layout_mut(MAIN).unwrap();
    let new = layout.open_tab();
    let copy = layout.open_tab_beside(a).unwrap();
    drop(layout);

    assert_eq!(main_group(&windows), vec![a]);
    assert!(!windows.in_group(MAIN, new));
    assert!(!windows.in_group(MAIN, copy));
}

#[test]
fn input_in_a_member_goes_to_every_member_and_elsewhere_only_to_its_own_session() {
    let mut windows = Windows::new();
    let [a, b, c] = open(&mut windows, MAIN, 3)[..] else {
        unreachable!()
    };
    for (tab, session) in [(a, 10), (b, 11), (c, 12)] {
        windows
            .attach_session(MAIN, tab, SessionId(session))
            .unwrap();
    }
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, c).unwrap();

    assert_eq!(
        windows.input_targets(SessionId(12)),
        vec![SessionId(10), SessionId(12)]
    );
    assert_eq!(windows.input_targets(SessionId(11)), vec![SessionId(11)]);
    // A session no tab shows yet gets its own input.
    assert_eq!(windows.input_targets(SessionId(77)), vec![SessionId(77)]);
}

#[test]
fn input_in_a_member_reaches_the_members_in_every_window() {
    let mut windows = Windows::new();
    let other = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    let [c, d] = open(&mut windows, other, 2)[..] else {
        unreachable!()
    };
    // Tab ids of different windows can be the same number: members are
    // named by window and tab.
    assert_eq!(a, c);
    for (window, tab, session) in [(MAIN, a, 1), (MAIN, b, 2), (other, c, 3), (other, d, 4)] {
        windows
            .attach_session(window, tab, SessionId(session))
            .unwrap();
    }
    windows.join_group(other, c).unwrap();
    windows.join_group(MAIN, b).unwrap();

    let targets = vec![SessionId(3), SessionId(2)];
    assert_eq!(windows.input_targets(SessionId(2)), targets);
    assert_eq!(windows.input_targets(SessionId(3)), targets);
    // Tabs outside the group reach only themselves, whichever window.
    assert_eq!(windows.input_targets(SessionId(1)), vec![SessionId(1)]);
    assert_eq!(windows.input_targets(SessionId(4)), vec![SessionId(4)]);
    assert_eq!(windows.group(), &[member(other, c), member(MAIN, b)]);
    assert_eq!(windows.window_group(other), vec![c]);
}

#[test]
fn a_member_without_a_session_yet_is_left_out_of_the_targets() {
    let mut windows = Windows::new();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, b).unwrap();

    assert_eq!(windows.input_targets(SessionId(1)), vec![SessionId(1)]);

    // Once its session is known, it receives.
    windows.attach_session(MAIN, b, SessionId(2)).unwrap();
    assert_eq!(
        windows.input_targets(SessionId(1)),
        vec![SessionId(1), SessionId(2)]
    );
}

#[test]
fn a_closed_member_tab_no_longer_receives_the_groups_input() {
    let mut windows = Windows::new();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    windows.attach_session(MAIN, b, SessionId(2)).unwrap();
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, b).unwrap();

    windows.layout_mut(MAIN).unwrap().close_tab(b).unwrap();

    assert_eq!(windows.input_targets(SessionId(1)), vec![SessionId(1)]);
    assert_eq!(windows.input_targets(SessionId(2)), vec![SessionId(2)]);
}

// Adding a whole pane to the group (#36).

#[test]
fn joining_a_pane_adds_each_of_its_tabs_not_yet_in_the_group() {
    let mut windows = Windows::new();
    let left = windows.layout(MAIN).unwrap().active_pane();
    let [a, b, c] = open(&mut windows, MAIN, 3)[..] else {
        unreachable!()
    };
    let right = windows
        .layout_mut(MAIN)
        .unwrap()
        .split(left, Split::Right)
        .unwrap();
    let d = open(&mut windows, MAIN, 1)[0];
    windows.join_group(MAIN, b).unwrap();
    windows.join_group(MAIN, d).unwrap();
    let before = revision(&windows, MAIN);

    // `c` moved to the right pane with the split.
    let joined = windows.join_pane(MAIN, right).unwrap();

    assert_eq!(joined, vec![c]);
    assert_eq!(main_group(&windows), vec![b, d, c]);
    assert!(revision(&windows, MAIN) > before);

    let joined = windows.join_pane(MAIN, left).unwrap();
    assert_eq!(joined, vec![a]);
    assert_eq!(main_group(&windows), vec![b, d, c, a]);
}

#[test]
fn joining_a_pane_whose_tabs_are_all_members_changes_nothing() {
    let mut windows = Windows::new();
    let pane = windows.layout(MAIN).unwrap().active_pane();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, b).unwrap();
    let before = revision(&windows, MAIN);

    assert_eq!(windows.join_pane(MAIN, pane).unwrap(), vec![]);
    assert_eq!(revision(&windows, MAIN), before);

    // Nor does joining an empty pane.
    let mut empty = Windows::new();
    let pane = empty.layout(MAIN).unwrap().active_pane();
    let before = revision(&empty, MAIN);
    assert_eq!(empty.join_pane(MAIN, pane).unwrap(), vec![]);
    assert_eq!(revision(&empty, MAIN), before);
}

#[test]
fn joining_an_unknown_pane_is_refused_without_changing_anything() {
    let mut windows = Windows::new();
    open(&mut windows, MAIN, 2);
    let before = revision(&windows, MAIN);

    let gone = PaneId(999);
    assert_eq!(
        windows.join_pane(MAIN, gone),
        Err(LayoutError::NoSuchPane(gone))
    );
    assert_eq!(revision(&windows, MAIN), before);
    assert_eq!(windows.group(), &[] as &[Member]);
}

// View tabs never join (#68).

#[test]
fn a_view_tab_cannot_join_the_multi_send_group() {
    let mut windows = Windows::new();
    let view = windows
        .layout_mut(MAIN)
        .unwrap()
        .open_view(preferences())
        .unwrap();
    let before = revision(&windows, MAIN);

    assert_eq!(
        windows.join_group(MAIN, view),
        Err(LayoutError::NotASession(view))
    );
    assert_eq!(windows.group(), &[] as &[Member]);
    assert_eq!(revision(&windows, MAIN), before);
}

#[test]
fn joining_a_pane_skips_its_view_tabs() {
    let mut windows = Windows::new();
    let pane = windows.layout(MAIN).unwrap().active_pane();
    let mut layout = windows.layout_mut(MAIN).unwrap();
    let a = layout.open_tab();
    layout.open_view(preferences()).unwrap();
    let b = layout.open_tab();
    drop(layout);

    assert_eq!(windows.join_pane(MAIN, pane).unwrap(), vec![a, b]);
    assert_eq!(main_group(&windows), vec![a, b]);
}

#[test]
fn a_view_tab_never_receives_a_sessions_input() {
    let mut windows = Windows::new();
    let a = open(&mut windows, MAIN, 1)[0];
    let view = windows
        .layout_mut(MAIN)
        .unwrap()
        .open_view(preferences())
        .unwrap();
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    windows.join_group(MAIN, a).unwrap();

    assert_eq!(
        windows.attach_session(MAIN, view, SessionId(2)),
        Err(LayoutError::NotASession(view))
    );
    assert_eq!(windows.input_targets(SessionId(1)), vec![SessionId(1)]);
    assert_eq!(windows.input_targets(SessionId(2)), vec![SessionId(2)]);
}

// Moving a session's tab to another window (#84).

#[test]
fn a_tab_moved_to_another_window_leaves_one_and_joins_the_other_with_its_session() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    windows.attach_session(MAIN, b, SessionId(2)).unwrap();

    let moved = windows.move_session(MAIN, a, session_window).unwrap();

    let main = windows.layout(MAIN).unwrap();
    assert_eq!(main.pane(main.active_pane()).unwrap().tabs, vec![b]);
    let other = windows.layout(session_window).unwrap();
    assert_eq!(other.active_tab(), Some(moved));
    assert_eq!(windows.session_tabs(MAIN), vec![(b, SessionId(2))]);
    assert_eq!(
        windows.session_tabs(session_window),
        vec![(moved, SessionId(1))]
    );
}

#[test]
fn a_tab_moves_back_to_the_main_window_as_its_newest_tab() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    let there = windows.move_session(MAIN, a, session_window).unwrap();

    let back = windows.move_session(session_window, there, MAIN).unwrap();

    let main = windows.layout(MAIN).unwrap();
    assert_eq!(main.pane(main.active_pane()).unwrap().tabs, vec![b, back]);
    assert_eq!(main.active_tab(), Some(back));
    assert_eq!(windows.session_tabs(MAIN), vec![(back, SessionId(1))]);
    assert_eq!(windows.session_tabs(session_window), vec![]);
}

#[test]
fn a_tab_whose_session_is_not_known_yet_moves_without_one() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];

    let moved = windows.move_session(MAIN, a, session_window).unwrap();

    assert_eq!(windows.session_tabs(session_window), vec![]);
    windows
        .attach_session(session_window, moved, SessionId(1))
        .unwrap();
    assert_eq!(
        windows.session_tabs(session_window),
        vec![(moved, SessionId(1))]
    );
}

#[test]
fn moving_a_member_keeps_it_in_the_group_in_its_place() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b, c] = open(&mut windows, MAIN, 3)[..] else {
        unreachable!()
    };
    for (tab, session) in [(a, 1), (b, 2), (c, 3)] {
        windows
            .attach_session(MAIN, tab, SessionId(session))
            .unwrap();
        windows.join_group(MAIN, tab).unwrap();
    }
    let group_revision = windows.group_revision();

    let moved = windows.move_session(MAIN, b, session_window).unwrap();

    assert_eq!(
        windows.group(),
        &[
            member(MAIN, a),
            member(session_window, moved),
            member(MAIN, c)
        ]
    );
    assert_eq!(windows.window_group(MAIN), vec![a, c]);
    assert_eq!(windows.window_group(session_window), vec![moved]);
    assert_eq!(windows.view(session_window).unwrap().group_size, 3);
    // Typing in it, or in another member, still reaches all three.
    let all = vec![SessionId(1), SessionId(2), SessionId(3)];
    assert_eq!(windows.input_targets(SessionId(2)), all);
    assert_eq!(windows.input_targets(SessionId(3)), all);
    // The same sessions are members: the group itself did not change.
    assert_eq!(windows.group_revision(), group_revision);

    let back = windows.move_session(session_window, moved, MAIN).unwrap();
    assert_eq!(windows.window_group(MAIN), vec![a, back, c]);
    assert_eq!(windows.input_targets(SessionId(1)), all);
}

#[test]
fn a_member_handed_to_the_main_window_stays_in_the_group() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    windows.attach_session(MAIN, b, SessionId(2)).unwrap();
    windows.join_group(MAIN, a).unwrap();
    windows.join_group(MAIN, b).unwrap();
    windows.move_session(MAIN, a, session_window).unwrap();

    let handed = windows.hand_to_main(session_window).unwrap();

    assert_eq!(windows.window_group(MAIN), vec![handed[0].tab, b]);
    assert_eq!(
        windows.input_targets(SessionId(2)),
        vec![SessionId(1), SessionId(2)]
    );
}

#[test]
fn the_group_revision_rises_whenever_the_group_changes_and_only_then() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    let c = open(&mut windows, session_window, 1)[0];
    let mut last = windows.group_revision();
    let mut rose = |windows: &Windows| {
        let now = windows.group_revision();
        let rose = now > last;
        last = now;
        rose
    };

    windows.join_group(MAIN, a).unwrap();
    assert!(rose(&windows), "a tab joined");
    windows.join_group(session_window, c).unwrap();
    assert!(rose(&windows), "a session window's tab joined");
    windows.join_group(MAIN, a).unwrap();
    windows.leave_group(MAIN, b).unwrap();
    open(&mut windows, MAIN, 1);
    windows.attach_session(MAIN, b, SessionId(2)).unwrap();
    assert!(!rose(&windows), "nothing changed the group");
    windows.join_group(MAIN, b).unwrap();
    assert!(rose(&windows));
    windows.leave_group(MAIN, b).unwrap();
    assert!(rose(&windows), "a tab left");
    windows.layout_mut(MAIN).unwrap().close_tab(a).unwrap();
    assert!(rose(&windows), "a member's tab closed");
    windows.close_window(session_window).unwrap();
    assert!(rose(&windows), "a member's window closed");
    windows.join_group(MAIN, b).unwrap();
    rose(&windows);
    windows.end_group();
    assert!(rose(&windows), "the group ended");
    windows.end_group();
    assert!(!rose(&windows), "ending no group changes nothing");
}

#[test]
fn a_paste_goes_to_the_members_of_every_window_the_one_pasted_in_first() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b, c, d] = open(&mut windows, MAIN, 4)[..] else {
        unreachable!()
    };
    for (tab, session) in [(a, 1), (b, 2), (c, 3)] {
        windows
            .attach_session(MAIN, tab, SessionId(session))
            .unwrap();
    }
    for tab in [a, b, c, d] {
        windows.join_group(MAIN, tab).unwrap();
    }
    let there = windows.move_session(MAIN, b, session_window).unwrap();

    // The one pasted in first, then the others in the order they joined;
    // a member whose session is not known yet (d) gets nothing.
    assert_eq!(
        windows.paste_members(MAIN, c),
        vec![
            (member(MAIN, c), SessionId(3)),
            (member(MAIN, a), SessionId(1)),
            (member(session_window, there), SessionId(2)),
        ]
    );
    assert_eq!(
        windows.paste_members(session_window, there),
        vec![
            (member(session_window, there), SessionId(2)),
            (member(MAIN, a), SessionId(1)),
            (member(MAIN, c), SessionId(3)),
        ]
    );
    // A tab outside the group pastes into its own session only.
    windows.leave_group(MAIN, a).unwrap();
    assert_eq!(windows.paste_members(MAIN, a), vec![]);
}

#[test]
fn a_members_lines_go_to_the_tab_showing_its_session_now() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [a, b] = open(&mut windows, MAIN, 2)[..] else {
        unreachable!()
    };
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    windows.attach_session(MAIN, b, SessionId(2)).unwrap();
    windows.join_group(MAIN, a).unwrap();

    assert_eq!(windows.member_showing(SessionId(1)), Some(member(MAIN, a)));
    let there = windows.move_session(MAIN, a, session_window).unwrap();
    assert_eq!(
        windows.member_showing(SessionId(1)),
        Some(member(session_window, there))
    );
    // Not a member, or unknown: nobody.
    assert_eq!(windows.member_showing(SessionId(2)), None);
    assert_eq!(windows.member_showing(SessionId(9)), None);
}

#[test]
fn moving_a_view_tab_or_to_the_same_or_an_unknown_window_is_refused() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    let view = windows
        .layout_mut(MAIN)
        .unwrap()
        .open_view(preferences())
        .unwrap();
    let before = revision(&windows, MAIN);

    assert_eq!(
        windows.move_session(MAIN, view, session_window),
        Err(LayoutError::NotASession(view))
    );
    assert_eq!(
        windows.move_session(MAIN, a, MAIN),
        Err(LayoutError::SameWindow)
    );
    assert_eq!(
        windows.move_session(MAIN, a, WindowId(99)),
        Err(LayoutError::NoSuchWindow(WindowId(99)))
    );
    assert_eq!(
        windows.move_session(MAIN, TabId(99), session_window),
        Err(LayoutError::NoSuchTab(TabId(99)))
    );
    assert_eq!(revision(&windows, MAIN), before);
    assert_eq!(windows.session_tabs(session_window), vec![]);
}

// What a session window's page starts from, and a window gone without
// docking back (#84).

#[test]
fn a_session_windows_page_takes_its_session_with_the_snapshot_kept_for_it() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    // Not moved yet: nothing to take.
    assert_eq!(windows.take_sessions(session_window), vec![]);

    let moved = windows.move_session(MAIN, a, session_window).unwrap();
    windows.keep_snapshot(session_window, moved, "screen".into());

    assert_eq!(
        windows.take_sessions(session_window),
        vec![Taken {
            tab: moved,
            session: SessionId(1),
            snapshot: Some("screen".into()),
        }]
    );
    // Taken again (the page reloaded): the snapshot was used up.
    assert_eq!(
        windows
            .take_sessions(session_window)
            .into_iter()
            .map(|taken| taken.snapshot)
            .collect::<Vec<_>>(),
        vec![None]
    );
}

// Several sessions in a session window (#85).

/// Opens a session tab for each of `sessions` in the main window and
/// moves each into `window`; returns their tabs there.
fn move_into(windows: &mut Windows, window: WindowId, sessions: &[u64]) -> Vec<TabId> {
    sessions
        .iter()
        .map(|&session| {
            let tab = open(windows, MAIN, 1)[0];
            windows
                .attach_session(MAIN, tab, SessionId(session))
                .unwrap();
            windows.move_session(MAIN, tab, window).unwrap()
        })
        .collect()
}

#[test]
fn a_session_moved_into_an_open_session_window_becomes_another_tab_there() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();

    let [first, second] = move_into(&mut windows, session_window, &[1, 2])[..] else {
        unreachable!()
    };

    let layout = windows.layout(session_window).unwrap();
    assert_eq!(
        layout.pane(layout.active_pane()).unwrap().tabs,
        vec![first, second]
    );
    assert_eq!(layout.active_tab(), Some(second));
    assert_eq!(
        windows.session_tabs(session_window),
        vec![(first, SessionId(1)), (second, SessionId(2))]
    );
}

#[test]
fn a_session_moves_from_one_session_window_into_another() {
    let mut windows = Windows::new();
    let left = windows.open_session_window();
    let right = windows.open_session_window();
    let there = move_into(&mut windows, left, &[1])[0];
    let leaving = move_into(&mut windows, right, &[2])[0];

    let arrived = windows.move_session(right, leaving, left).unwrap();

    assert_eq!(
        windows.session_tabs(left),
        vec![(there, SessionId(1)), (arrived, SessionId(2))]
    );
    // Its last tab moved out: the window it left closed.
    assert!(windows.layout(right).is_none());
    assert_eq!(windows.windows(), vec![MAIN, left]);
}

#[test]
fn a_session_window_whose_last_tab_moves_out_closes_and_one_with_more_stays() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [first, second] = move_into(&mut windows, session_window, &[1, 2])[..] else {
        unreachable!()
    };
    windows.keep_snapshot(session_window, second, "kept".into());

    windows.move_session(session_window, first, MAIN).unwrap();
    assert_eq!(
        windows.session_tabs(session_window),
        vec![(second, SessionId(2))]
    );

    windows.move_session(session_window, second, MAIN).unwrap();
    assert!(windows.layout(session_window).is_none());
    assert_eq!(windows.windows(), vec![MAIN]);
    assert_eq!(
        windows
            .session_tabs(MAIN)
            .into_iter()
            .map(|(_, session)| session)
            .collect::<Vec<_>>(),
        vec![SessionId(1), SessionId(2)]
    );
    // The main window's last tab moving out leaves it open.
    let back = windows.session_tabs(MAIN);
    let other = windows.open_session_window();
    for (tab, _) in back {
        windows.move_session(MAIN, tab, other).unwrap();
    }
    assert!(windows.layout(MAIN).is_some());
}

#[test]
fn a_session_windows_page_takes_every_session_in_reading_order() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let [first, second] = move_into(&mut windows, session_window, &[1, 2])[..] else {
        unreachable!()
    };
    windows.keep_snapshot(session_window, first, "one".into());
    windows.keep_snapshot(session_window, second, "two".into());

    assert_eq!(
        windows.take_sessions(session_window),
        vec![
            Taken {
                tab: first,
                session: SessionId(1),
                snapshot: Some("one".into()),
            },
            Taken {
                tab: second,
                session: SessionId(2),
                snapshot: Some("two".into()),
            },
        ]
    );
}

#[test]
fn an_open_session_window_takes_the_tab_moved_into_it_once_with_its_snapshot() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let first = move_into(&mut windows, session_window, &[1])[0];
    windows.keep_snapshot(session_window, first, "one".into());
    assert_eq!(windows.take_sessions(session_window).len(), 1);
    let second = move_into(&mut windows, session_window, &[2])[0];
    windows.keep_snapshot(session_window, second, "two".into());

    assert_eq!(
        windows.take_tab(session_window, second),
        Some(Taken {
            tab: second,
            session: SessionId(2),
            snapshot: Some("two".into()),
        })
    );
    // Taken again, or a tab it does not show: no snapshot, or nothing.
    assert_eq!(
        windows
            .take_tab(session_window, second)
            .map(|taken| taken.snapshot),
        Some(None)
    );
    assert_eq!(windows.take_tab(session_window, TabId(99)), None);
    assert_eq!(windows.take_tab(WindowId(99), second), None);
}

#[test]
fn session_windows_are_named_by_the_session_their_active_tab_shows() {
    let mut windows = Windows::new();
    let left = windows.open_session_window();
    let right = windows.open_session_window();
    let [first, _second] = move_into(&mut windows, left, &[1, 2])[..] else {
        unreachable!()
    };
    assert_eq!(
        windows.session_windows(),
        vec![(left, Some(SessionId(2))), (right, None)]
    );

    windows.layout_mut(left).unwrap().activate(first).unwrap();

    assert_eq!(
        windows.session_windows(),
        vec![(left, Some(SessionId(1))), (right, None)]
    );
}

#[test]
fn a_session_window_gone_hands_every_session_to_the_main_window() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    move_into(&mut windows, session_window, &[1, 2]);

    let handed = windows.hand_to_main(session_window).unwrap();

    assert_eq!(
        handed.iter().map(|taken| taken.session).collect::<Vec<_>>(),
        vec![SessionId(1), SessionId(2)]
    );
    assert!(windows.layout(session_window).is_none());
}

#[test]
fn a_session_window_gone_hands_its_sessions_to_the_main_window_and_closes() {
    let mut windows = Windows::new();
    let session_window = windows.open_session_window();
    let a = open(&mut windows, MAIN, 1)[0];
    windows.attach_session(MAIN, a, SessionId(1)).unwrap();
    let moved = windows.move_session(MAIN, a, session_window).unwrap();
    windows.keep_snapshot(session_window, moved, "never taken".into());

    let handed = windows.hand_to_main(session_window).unwrap();

    let [back] = &handed[..] else {
        panic!("{handed:?}")
    };
    assert_eq!(back.session, SessionId(1));
    // Its page never took the snapshot: the main window gets it.
    assert_eq!(back.snapshot.as_deref(), Some("never taken"));
    assert_eq!(windows.session_tabs(MAIN), vec![(back.tab, SessionId(1))]);
    assert!(windows.layout(session_window).is_none());
    assert_eq!(
        windows.hand_to_main(session_window),
        Err(LayoutError::NoSuchWindow(session_window))
    );
    assert_eq!(windows.hand_to_main(MAIN), Err(LayoutError::MainWindow));
}

// The terminal last used, whichever window it is in (#86).

#[test]
fn the_terminal_last_used_is_the_one_snippets_go_to_whichever_window_it_is_in() {
    let mut windows = Windows::new();
    assert_eq!(windows.last_used(), None);
    let session_window = windows.open_session_window();
    let there = move_into(&mut windows, session_window, &[1])[0];
    let here = open(&mut windows, MAIN, 1)[0];
    windows.attach_session(MAIN, here, SessionId(2)).unwrap();

    assert_eq!(
        windows.session_of(session_window, there),
        Some(SessionId(1))
    );
    assert_eq!(windows.session_of(MAIN, there), None);
    windows.use_tab(session_window, there).unwrap();
    assert_eq!(
        windows.last_used(),
        Some(Used {
            window: session_window,
            tab: there,
            session: SessionId(1)
        })
    );

    windows.use_tab(MAIN, here).unwrap();
    assert_eq!(
        windows.last_used(),
        Some(Used {
            window: MAIN,
            tab: here,
            session: SessionId(2)
        })
    );
}

#[test]
fn the_terminal_last_used_follows_its_session_to_another_window_and_goes_with_it() {
    let mut windows = Windows::new();
    let tab = open(&mut windows, MAIN, 1)[0];
    windows.attach_session(MAIN, tab, SessionId(1)).unwrap();
    windows.use_tab(MAIN, tab).unwrap();
    let session_window = windows.open_session_window();

    let moved = windows.move_session(MAIN, tab, session_window).unwrap();
    assert_eq!(
        windows.last_used(),
        Some(Used {
            window: session_window,
            tab: moved,
            session: SessionId(1)
        })
    );

    let back = windows.move_session(session_window, moved, MAIN).unwrap();
    assert_eq!(
        windows.last_used().map(|used| (used.window, used.tab)),
        Some((MAIN, back))
    );

    windows.layout_mut(MAIN).unwrap().close_tab(back).unwrap();
    assert_eq!(windows.last_used(), None);
}

#[test]
fn using_a_view_tab_a_tab_without_a_session_or_an_unknown_window_changes_nothing() {
    let mut windows = Windows::new();
    let tab = open(&mut windows, MAIN, 1)[0];
    windows.attach_session(MAIN, tab, SessionId(1)).unwrap();
    windows.use_tab(MAIN, tab).unwrap();
    let view = windows
        .layout_mut(MAIN)
        .unwrap()
        .open_view(preferences())
        .unwrap();
    let opening = open(&mut windows, MAIN, 1)[0];

    assert_eq!(
        windows.use_tab(MAIN, view),
        Err(LayoutError::NotASession(view))
    );
    assert_eq!(
        windows.use_tab(WindowId(9), tab),
        Err(LayoutError::NoSuchWindow(WindowId(9)))
    );
    // Its session is still opening: nothing to send to yet.
    windows.use_tab(MAIN, opening).unwrap();

    assert_eq!(windows.last_used().map(|used| used.tab), Some(tab));
}

// Closing rules (#88).

#[test]
fn every_session_windows_sessions_are_listed_window_by_window() {
    let mut windows = Windows::new();
    let main_tab = open(&mut windows, MAIN, 1)[0];
    windows
        .attach_session(MAIN, main_tab, SessionId(9))
        .unwrap();
    let first = windows.open_session_window();
    let second = windows.open_session_window();
    let [b] = move_into(&mut windows, second, &[3])[..] else {
        unreachable!()
    };
    let [a1, a2] = move_into(&mut windows, first, &[1, 2])[..] else {
        unreachable!()
    };

    // The main window's own are not listed: its page knows them.
    assert_eq!(
        windows.session_window_sessions(),
        vec![
            (member(first, a1), SessionId(1)),
            (member(first, a2), SessionId(2)),
            (member(second, b), SessionId(3)),
        ]
    );
}

#[test]
fn closing_sessions_closes_their_tabs_in_session_windows_and_the_windows_left_empty() {
    let mut windows = Windows::new();
    let main_tab = open(&mut windows, MAIN, 1)[0];
    windows
        .attach_session(MAIN, main_tab, SessionId(9))
        .unwrap();
    let first = windows.open_session_window();
    let second = windows.open_session_window();
    let [_, kept] = move_into(&mut windows, first, &[1, 2])[..] else {
        unreachable!()
    };
    move_into(&mut windows, second, &[3]);
    let before = revision(&windows, first);

    let closed = windows.close_sessions(&[SessionId(1), SessionId(3), SessionId(9)]);

    assert_eq!(closed, vec![second]);
    assert_eq!(windows.session_tabs(first), vec![(kept, SessionId(2))]);
    assert!(revision(&windows, first) > before);
    assert!(windows.layout(second).is_none());
    // The main window's tabs are its page's to close.
    assert_eq!(windows.session_tabs(MAIN), vec![(main_tab, SessionId(9))]);
}

#[test]
fn closing_a_member_in_a_session_window_drops_it_from_the_group() {
    let mut windows = Windows::new();
    let window = windows.open_session_window();
    let [a, b] = move_into(&mut windows, window, &[1, 2])[..] else {
        unreachable!()
    };
    windows.join_group(window, a).unwrap();
    windows.join_group(window, b).unwrap();
    let group = windows.group_revision();

    windows.close_sessions(&[SessionId(1)]);

    assert_eq!(windows.window_group(window), vec![b]);
    assert!(windows.group_revision() > group);
}
