//! The window's session layout: tabs in panes, panes split side by side or
//! one above the other. The multi-send group spans windows: `windows.rs`.

use netterm_core::layout::{Layout, LayoutError, Node, PaneId, Split, TabId, TabKind, View};
use serde_json::json;

/// The tabs of `pane`, in order.
fn tabs(layout: &Layout, pane: PaneId) -> Vec<TabId> {
    layout.pane(pane).expect("the pane exists").tabs.clone()
}

/// The tab shown in `pane`, if any.
fn shown(layout: &Layout, pane: PaneId) -> Option<TabId> {
    layout.pane(pane).expect("the pane exists").active
}

/// Opens `count` tabs in the active pane.
fn open(layout: &mut Layout, count: usize) -> Vec<TabId> {
    (0..count).map(|_| layout.open_tab()).collect()
}

#[test]
fn a_new_layout_has_one_empty_pane() {
    let layout = Layout::new();

    let pane = layout.active_pane();
    assert_eq!(layout.panes(), vec![pane]);
    assert_eq!(tabs(&layout, pane), vec![]);
    assert_eq!(layout.active_tab(), None);
    assert_eq!(layout.visible_tabs(), vec![]);
}

#[test]
fn each_new_tab_opens_at_the_end_of_the_active_pane_and_is_shown() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();

    let first = layout.open_tab();
    let second = layout.open_tab();

    assert_ne!(first, second);
    assert_eq!(tabs(&layout, pane), vec![first, second]);
    assert_eq!(layout.active_tab(), Some(second));
    assert_eq!(layout.pane_of(first), Some(pane));
}

#[test]
fn a_tab_opened_beside_another_goes_right_after_it() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };

    let copy = layout.open_tab_beside(a).unwrap();

    assert_eq!(tabs(&layout, pane), vec![a, copy, b]);
    assert_eq!(layout.active_tab(), Some(copy));
}

#[test]
fn next_and_previous_go_round_the_tabs_of_the_active_pane() {
    let mut layout = Layout::new();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };
    assert_eq!(layout.active_tab(), Some(c));

    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(a));
    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(b));
    layout.previous_tab();
    layout.previous_tab();
    assert_eq!(layout.active_tab(), Some(c));
}

#[test]
fn next_and_previous_from_the_connect_view_go_to_the_first_and_last_tab() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let [a, _, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };

    layout.show_connect_view(pane).unwrap();
    assert_eq!(layout.active_tab(), None);
    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(a));

    layout.show_connect_view(pane).unwrap();
    layout.previous_tab();
    assert_eq!(layout.active_tab(), Some(c));
}

#[test]
fn next_and_previous_without_tabs_change_nothing() {
    let mut layout = Layout::new();
    let revision = layout.revision();

    layout.next_tab();
    layout.previous_tab();

    assert_eq!(layout.active_tab(), None);
    assert_eq!(layout.revision(), revision);
}

#[test]
fn closing_the_shown_tab_shows_its_right_neighbour_else_its_left_one() {
    let mut layout = Layout::new();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };
    layout.activate(b).unwrap();

    layout.close_tab(b).unwrap();
    assert_eq!(layout.active_tab(), Some(c));

    layout.close_tab(c).unwrap();
    assert_eq!(layout.active_tab(), Some(a));
}

#[test]
fn closing_a_tab_out_of_sight_keeps_the_shown_one() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };

    layout.close_tab(a).unwrap();

    assert_eq!(tabs(&layout, pane), vec![b, c]);
    assert_eq!(layout.active_tab(), Some(c));
}

#[test]
fn closing_the_last_tab_leaves_the_empty_pane_showing_the_connect_view() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let tab = layout.open_tab();

    layout.close_tab(tab).unwrap();

    assert_eq!(layout.panes(), vec![pane]);
    assert_eq!(tabs(&layout, pane), vec![]);
    assert_eq!(layout.active_tab(), None);
    assert_eq!(layout.pane_of(tab), None);
}

#[test]
fn dismissing_a_tab_removes_it_and_shows_the_connect_view_in_its_pane() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };

    layout.dismiss_tab(b).unwrap();

    assert_eq!(tabs(&layout, pane), vec![a]);
    assert_eq!(layout.active_tab(), None);
}

#[test]
fn tabs_can_be_reordered_one_step_at_a_time() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };

    layout.move_tab_by(c, -1).unwrap();
    assert_eq!(tabs(&layout, pane), vec![a, c, b]);
    layout.move_tab_by(c, -1).unwrap();
    assert_eq!(tabs(&layout, pane), vec![c, a, b]);
    // Already first: stays there.
    layout.move_tab_by(c, -1).unwrap();
    assert_eq!(tabs(&layout, pane), vec![c, a, b]);
    layout.move_tab_by(a, 5).unwrap();
    assert_eq!(tabs(&layout, pane), vec![c, b, a]);
    assert_eq!(layout.active_tab(), Some(c));
}

#[test]
fn a_tab_can_be_moved_to_any_position_in_its_pane() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };

    // The index counts the other tabs: before the first of them.
    layout.move_tab(c, pane, 0).unwrap();
    assert_eq!(tabs(&layout, pane), vec![c, a, b]);
    layout.move_tab(c, pane, 99).unwrap();
    assert_eq!(tabs(&layout, pane), vec![a, b, c]);
    layout.move_tab(a, pane, 1).unwrap();
    assert_eq!(tabs(&layout, pane), vec![b, a, c]);
}

#[test]
fn splitting_a_pane_with_several_tabs_moves_its_shown_tab_beside_it() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };

    let right = layout.split(left, Split::Right).unwrap();

    assert_eq!(layout.panes(), vec![left, right]);
    assert_eq!(tabs(&layout, left), vec![a]);
    assert_eq!(shown(&layout, left), Some(a));
    assert_eq!(tabs(&layout, right), vec![b]);
    assert_eq!(layout.active_pane(), right);
    assert_eq!(layout.active_tab(), Some(b));
    assert_eq!(layout.visible_tabs(), vec![a, b]);
    match layout.root() {
        Node::Split(split) => {
            assert_eq!(split.direction, Split::Right);
            assert_eq!(split.children.len(), 2);
        }
        other => panic!("expected a split, got {other:?}"),
    }
}

#[test]
fn splitting_a_pane_with_one_tab_adds_an_empty_pane_for_a_new_connection() {
    let mut layout = Layout::new();
    let top = layout.active_pane();
    let tab = layout.open_tab();

    let bottom = layout.split(top, Split::Down).unwrap();

    assert_eq!(layout.panes(), vec![top, bottom]);
    assert_eq!(tabs(&layout, top), vec![tab]);
    assert_eq!(tabs(&layout, bottom), vec![]);
    assert_eq!(layout.active_pane(), bottom);
    assert_eq!(layout.active_tab(), None);

    // The next session opens in the new pane.
    let next = layout.open_tab();
    assert_eq!(layout.pane_of(next), Some(bottom));
}

#[test]
fn splitting_again_the_same_way_adds_a_pane_beside_the_others() {
    let mut layout = Layout::new();
    let first = layout.active_pane();
    let second = layout.split(first, Split::Right).unwrap();

    let between = layout.split(first, Split::Right).unwrap();

    assert_eq!(layout.panes(), vec![first, between, second]);
    let Node::Split(split) = layout.root() else {
        panic!("expected a split")
    };
    assert_eq!(split.children.len(), 3);
}

#[test]
fn splitting_the_other_way_nests_the_panes() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let right = layout.split(left, Split::Right).unwrap();

    let below = layout.split(right, Split::Down).unwrap();

    assert_eq!(layout.panes(), vec![left, right, below]);
    let Node::Split(outer) = layout.root() else {
        panic!("expected a split")
    };
    assert_eq!(outer.direction, Split::Right);
    match &outer.children[..] {
        [Node::Pane(pane), Node::Split(inner)] => {
            assert_eq!(pane.id, left);
            assert_eq!(inner.direction, Split::Down);
            assert_eq!(inner.children.len(), 2);
        }
        other => panic!("unexpected children {other:?}"),
    }
}

#[test]
fn a_tab_can_be_moved_into_another_pane() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();
    assert_eq!(tabs(&layout, right), vec![c]);
    layout.activate(a).unwrap();

    layout.move_tab(a, right, 0).unwrap();

    assert_eq!(tabs(&layout, left), vec![b]);
    assert_eq!(shown(&layout, left), Some(b));
    assert_eq!(tabs(&layout, right), vec![a, c]);
    assert_eq!(layout.active_pane(), right);
    assert_eq!(layout.active_tab(), Some(a));
}

#[test]
fn moving_the_last_tab_out_of_a_pane_removes_the_pane() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();

    layout.move_tab(b, left, 99).unwrap();

    assert_eq!(layout.panes(), vec![left]);
    assert_eq!(tabs(&layout, left), vec![a, b]);
    assert!(matches!(layout.root(), Node::Pane(_)));
    assert_eq!(layout.pane(right), None);
}

#[test]
fn closing_the_last_tab_of_one_of_several_panes_removes_the_pane() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();
    let below = layout.split(right, Split::Down).unwrap();
    let c = layout.open_tab();
    assert_eq!(layout.pane_of(c), Some(below));

    layout.close_tab(c).unwrap();

    // The nested split with one pane left collapses into that pane.
    assert_eq!(layout.panes(), vec![left, right]);
    let Node::Split(split) = layout.root() else {
        panic!("expected a split")
    };
    assert!(split
        .children
        .iter()
        .all(|child| matches!(child, Node::Pane(_))));
    // The neighbour takes over.
    assert_eq!(layout.active_pane(), right);
    assert_eq!(layout.visible_tabs(), vec![a, b]);
}

#[test]
fn closing_a_pane_moves_its_tabs_to_its_neighbour() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();
    layout.move_tab(b, right, 0).unwrap();
    assert_eq!(tabs(&layout, right), vec![b, c]);

    layout.close_pane(right).unwrap();

    assert_eq!(layout.panes(), vec![left]);
    assert_eq!(tabs(&layout, left), vec![a, b, c]);
    assert_eq!(layout.active_pane(), left);
    assert_eq!(layout.active_tab(), Some(b));
}

#[test]
fn the_first_pane_closed_gives_its_tabs_to_the_pane_after_it() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();

    layout.close_pane(left).unwrap();

    assert_eq!(layout.panes(), vec![right]);
    assert_eq!(tabs(&layout, right), vec![a, b]);
    assert_eq!(layout.active_tab(), Some(b));
}

#[test]
fn the_last_pane_cannot_be_closed() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let tab = layout.open_tab();

    assert_eq!(layout.close_pane(pane), Err(LayoutError::LastPane));
    assert_eq!(tabs(&layout, pane), vec![tab]);
}

#[test]
fn showing_a_tab_in_another_pane_makes_that_pane_active() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, _] = open(&mut layout, 2)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();
    assert_eq!(layout.active_pane(), right);

    layout.activate(a).unwrap();
    assert_eq!(layout.active_pane(), left);
    assert_eq!(layout.active_tab(), Some(a));

    layout.activate_pane(right).unwrap();
    assert_eq!(layout.active_pane(), right);
}

#[test]
fn next_and_previous_stay_in_the_active_pane() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b, c] = open(&mut layout, 3)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Right).unwrap();
    assert_eq!(tabs(&layout, right), vec![c]);

    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(c));

    layout.activate_pane(left).unwrap();
    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(a));
    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(b));
}

#[test]
fn unknown_tabs_and_panes_are_refused_without_changing_anything() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    // A tab and a pane that existed once.
    let tab = layout.open_tab();
    layout.close_tab(tab).unwrap();
    let other = layout.open_tab();
    let missing_pane = layout.split(pane, Split::Right).unwrap();
    layout.close_pane(missing_pane).unwrap();
    let before = serde_json::to_value(&layout).unwrap();
    let before_revision = layout.revision();

    assert_eq!(layout.activate(tab), Err(LayoutError::NoSuchTab(tab)));
    assert_eq!(layout.close_tab(tab), Err(LayoutError::NoSuchTab(tab)));
    assert_eq!(layout.dismiss_tab(tab), Err(LayoutError::NoSuchTab(tab)));
    assert_eq!(
        layout.open_tab_beside(tab),
        Err(LayoutError::NoSuchTab(tab))
    );
    assert_eq!(layout.move_tab_by(tab, 1), Err(LayoutError::NoSuchTab(tab)));
    assert_eq!(
        layout.move_tab(tab, pane, 0),
        Err(LayoutError::NoSuchTab(tab))
    );
    assert_eq!(
        layout.move_tab(other, missing_pane, 0),
        Err(LayoutError::NoSuchPane(missing_pane))
    );
    assert_eq!(
        layout.split(missing_pane, Split::Down),
        Err(LayoutError::NoSuchPane(missing_pane))
    );
    assert_eq!(
        layout.activate_pane(missing_pane),
        Err(LayoutError::NoSuchPane(missing_pane))
    );
    assert_eq!(
        layout.show_connect_view(missing_pane),
        Err(LayoutError::NoSuchPane(missing_pane))
    );

    assert_eq!(serde_json::to_value(&layout).unwrap(), before);
    assert_eq!(layout.revision(), before_revision);
}

#[test]
fn every_change_raises_the_revision() {
    let mut layout = Layout::new();
    let mut last = layout.revision();
    let mut changed = |layout: &Layout| {
        assert!(layout.revision() > last, "the revision did not rise");
        last = layout.revision();
    };

    let pane = layout.active_pane();
    let a = layout.open_tab();
    changed(&layout);
    let b = layout.open_tab_beside(a).unwrap();
    changed(&layout);
    layout.next_tab();
    changed(&layout);
    layout.move_tab_by(b, -1).unwrap();
    changed(&layout);
    let other = layout.split(pane, Split::Right).unwrap();
    changed(&layout);
    layout.activate(a).unwrap();
    changed(&layout);
    layout.close_pane(other).unwrap();
    changed(&layout);
    layout.dismiss_tab(a).unwrap();
    changed(&layout);
}

#[test]
fn twenty_tabs_can_be_open_at_once() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();

    let opened = open(&mut layout, 20);

    assert_eq!(tabs(&layout, pane), opened);
    for _ in 0..20 {
        layout.next_tab();
    }
    assert_eq!(layout.active_tab(), opened.last().copied());
}

#[test]
fn the_layout_reaches_the_interface_as_a_tree() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let [a, b] = open(&mut layout, 2)[..] else {
        unreachable!()
    };
    let right = layout.split(left, Split::Down).unwrap();

    assert_eq!(
        serde_json::to_value(&layout).unwrap(),
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
            "kinds": {
                a.0.to_string(): { "kind": "session" },
                b.0.to_string(): { "kind": "session" },
            },
            "revision": layout.revision(),
        })
    );
}

// View tabs (#68): a tab holds a session or a view.

fn preferences() -> View {
    View::new("preferences", None)
}

#[test]
fn a_view_opens_as_a_tab_at_the_end_of_the_active_pane() {
    let mut layout = Layout::new();
    let pane = layout.active_pane();
    let session = layout.open_tab();

    let view = layout.open_view(preferences()).unwrap();

    assert_eq!(tabs(&layout, pane), vec![session, view]);
    assert_eq!(layout.active_tab(), Some(view));
    assert_eq!(layout.tab_kind(view), Some(&TabKind::View(preferences())));
    assert_eq!(layout.tab_kind(session), Some(&TabKind::Session));
}

#[test]
fn opening_a_view_that_is_open_brings_its_tab_forward() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let view = layout.open_view(preferences()).unwrap();
    layout.open_tab();
    let right = layout.split(left, Split::Right).unwrap();
    assert_eq!(layout.active_pane(), right);

    let again = layout.open_view(preferences()).unwrap();

    assert_eq!(again, view);
    assert_eq!(layout.active_pane(), left);
    assert_eq!(shown(&layout, left), Some(view));
    assert_eq!(tabs(&layout, left), vec![view]);
}

#[test]
fn the_same_view_about_another_subject_is_another_tab() {
    let mut layout = Layout::new();
    let r1 = layout
        .open_view(View::new("config-history", Some("r1.yaml".into())))
        .unwrap();
    let r2 = layout
        .open_view(View::new("config-history", Some("r2.yaml".into())))
        .unwrap();

    assert_ne!(r1, r2);
    assert_eq!(
        layout
            .open_view(View::new("config-history", Some("r1.yaml".into())))
            .unwrap(),
        r1
    );
}

#[test]
fn a_closed_view_opens_again_as_a_new_tab() {
    let mut layout = Layout::new();
    let view = layout.open_view(preferences()).unwrap();
    layout.close_tab(view).unwrap();

    let again = layout.open_view(preferences()).unwrap();

    assert_ne!(again, view);
    assert_eq!(layout.tab_kind(view), None);
}

#[test]
fn view_tabs_move_split_close_and_go_round_like_session_tabs() {
    let mut layout = Layout::new();
    let left = layout.active_pane();
    let a = layout.open_tab();
    let view = layout.open_view(preferences()).unwrap();
    let b = layout.open_tab();

    layout.previous_tab();
    assert_eq!(layout.active_tab(), Some(view));
    layout.next_tab();
    assert_eq!(layout.active_tab(), Some(b));

    layout.move_tab_by(view, -1).unwrap();
    assert_eq!(tabs(&layout, left), vec![view, a, b]);

    layout.activate(view).unwrap();
    let right = layout.split(left, Split::Right).unwrap();
    assert_eq!(tabs(&layout, right), vec![view]);
    assert_eq!(tabs(&layout, left), vec![a, b]);

    layout.move_tab(view, left, 1).unwrap();
    assert_eq!(tabs(&layout, left), vec![a, view, b]);
    assert_eq!(layout.panes(), vec![left]);

    layout.close_tab(view).unwrap();
    assert_eq!(tabs(&layout, left), vec![a, b]);
    assert_eq!(layout.active_tab(), Some(b));
}

#[test]
fn the_interface_sees_each_tabs_kind_and_subject() {
    let mut layout = Layout::new();
    let session = layout.open_tab();
    let prefs = layout.open_view(preferences()).unwrap();
    let history = layout
        .open_view(View::new("config-history", Some("r1.yaml".into())))
        .unwrap();

    let view = serde_json::to_value(&layout).unwrap();
    assert_eq!(
        view["kinds"],
        json!({
            session.0.to_string(): { "kind": "session" },
            prefs.0.to_string(): { "kind": "view", "view": "preferences", "subject": null },
            history.0.to_string(): { "kind": "view", "view": "config-history", "subject": "r1.yaml" },
        })
    );
}
