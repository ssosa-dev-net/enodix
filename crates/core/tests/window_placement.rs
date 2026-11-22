//! Where a new session window opens (#90): 40 logical pixels right and
//! down of the window it comes from, fitted inside that window's monitor's
//! work area, worked out in physical pixels so that the position names one
//! monitor whatever each monitor's display scaling is.

use enodix_core::layout::{
    place_session_window, Frame, PhysicalRect, Placement, SESSION_WINDOW_MIN_SIZE,
};

/// The invisible frame of a window with a shadow and no title bar at 150%.
const SHADOW_FRAME: Frame = Frame { left: 11, top: 0 };

/// A 3440x1440 primary at 0,0 with a 40 px taskbar.
const PRIMARY: PhysicalRect = PhysicalRect {
    x: 0,
    y: 0,
    width: 3440,
    height: 1400,
};

/// A 2880x1800 laptop screen to the right of the primary, its work area
/// 48 px short of the bottom.
const LAPTOP: PhysicalRect = PhysicalRect {
    x: 3440,
    y: 0,
    width: 2880,
    height: 1752,
};

/// A 1920x1080 monitor to the left of and below the primary's origin.
const LEFT: PhysicalRect = PhysicalRect {
    x: -1920,
    y: 200,
    width: 1920,
    height: 1040,
};

fn inside(placed: Placement, scale: f64, area: PhysicalRect) -> bool {
    let width = (placed.width * scale).round() as i64;
    let height = (placed.height * scale).round() as i64;
    let (x, y) = (i64::from(placed.x), i64::from(placed.y));
    x >= i64::from(area.x)
        && y >= i64::from(area.y)
        && x + width <= i64::from(area.x) + i64::from(area.width)
        && y + height <= i64::from(area.y) + i64::from(area.height)
}

#[test]
fn at_scale_1_it_opens_40_pixels_right_and_down() {
    let placed = place_session_window(
        Some((100, 120)),
        1.0,
        Some(PRIMARY),
        (900.0, 600.0),
        Frame::default(),
    );
    assert_eq!(
        placed,
        Placement {
            x: 140,
            y: 160,
            width: 900.0,
            height: 600.0
        }
    );
    assert!(inside(placed, 1.0, PRIMARY));
}

#[test]
fn at_scale_1_25_the_offset_is_40_logical_pixels() {
    let placed = place_session_window(
        Some((3500, 100)),
        1.25,
        Some(LAPTOP),
        (900.0, 600.0),
        Frame::default(),
    );
    assert_eq!(
        placed,
        Placement {
            x: 3550,
            y: 150,
            width: 900.0,
            height: 600.0
        }
    );
    assert!(inside(placed, 1.25, LAPTOP));
}

#[test]
fn at_scale_1_5_on_a_monitor_right_of_the_primary_it_stays_on_it() {
    // The case of the issue: x 3600 on a monitor scaled more than the
    // primary. Logical 3600 / 1.5 = 2400 would fall on the primary.
    let placed = place_session_window(
        Some((3600, 100)),
        1.5,
        Some(LAPTOP),
        (900.0, 600.0),
        Frame::default(),
    );
    assert_eq!(
        placed,
        Placement {
            x: 3660,
            y: 160,
            width: 900.0,
            height: 600.0
        }
    );
    assert!(inside(placed, 1.5, LAPTOP));
}

#[test]
fn near_the_bottom_right_corner_it_is_fitted_inside_the_work_area() {
    for scale in [1.0, 1.25, 1.5] {
        let placed = place_session_window(
            Some((LAPTOP.x + 2800, 1700)),
            scale,
            Some(LAPTOP),
            (900.0, 600.0),
            Frame::default(),
        );
        assert!(inside(placed, scale, LAPTOP), "{scale}: {placed:?}");
        let right = i64::from(LAPTOP.x) + i64::from(LAPTOP.width);
        let width = (900.0 * scale).round() as i64;
        assert_eq!(i64::from(placed.x), right - width, "{scale}");
        let bottom = i64::from(LAPTOP.height);
        let height = (600.0 * scale).round() as i64;
        assert_eq!(i64::from(placed.y), bottom - height, "{scale}");
        assert_eq!((placed.width, placed.height), (900.0, 600.0));
    }
}

#[test]
fn on_a_monitor_that_does_not_start_at_0_0_it_stays_on_it() {
    for scale in [1.0, 1.25, 1.5] {
        // A window above and left of the work area's corner is pushed in.
        let placed = place_session_window(
            Some((-2000, 100)),
            scale,
            Some(LEFT),
            (800.0, 500.0),
            Frame::default(),
        );
        assert_eq!((placed.x, placed.y), (LEFT.x, LEFT.y), "{scale}");
        assert!(inside(placed, scale, LEFT), "{scale}: {placed:?}");

        let placed = place_session_window(
            Some((-1800, 300)),
            scale,
            Some(LEFT),
            (400.0, 300.0),
            Frame::default(),
        );
        let offset = (40.0 * scale).round() as i32;
        assert_eq!((placed.x, placed.y), (-1800 + offset, 300 + offset));
        assert!(inside(placed, scale, LEFT), "{scale}: {placed:?}");
    }
}

#[test]
fn a_window_larger_than_the_work_area_is_shrunk_to_it() {
    for scale in [1.0, 1.25, 1.5] {
        let placed = place_session_window(
            Some((LEFT.x, 300)),
            scale,
            Some(LEFT),
            (5000.0, 4000.0),
            Frame::default(),
        );
        assert_eq!((placed.x, placed.y), (LEFT.x, LEFT.y), "{scale}");
        assert_eq!(placed.width, f64::from(LEFT.width) / scale);
        assert_eq!(placed.height, f64::from(LEFT.height) / scale);
        assert!(inside(placed, scale, LEFT), "{scale}: {placed:?}");
    }
}

#[test]
fn without_a_monitor_it_is_offset_and_not_fitted() {
    let placed = place_session_window(
        Some((5000, 5000)),
        1.5,
        None,
        (900.0, 600.0),
        Frame::default(),
    );
    assert_eq!(
        placed,
        Placement {
            x: 5060,
            y: 5060,
            width: 900.0,
            height: 600.0
        }
    );
}

#[test]
fn without_the_window_s_position_it_opens_near_the_work_area_s_corner() {
    let placed = place_session_window(None, 1.25, Some(LAPTOP), (900.0, 600.0), Frame::default());
    assert_eq!((placed.x, placed.y), (LAPTOP.x + 50, LAPTOP.y + 50));
}

#[test]
fn near_the_edges_its_inner_area_is_fitted_and_its_invisible_frame_left_out() {
    let placed = place_session_window(
        Some((LAPTOP.x + 2800, 1700)),
        1.5,
        Some(LAPTOP),
        (900.0, 600.0),
        SHADOW_FRAME,
    );
    // The inner area ends at the work area's right and bottom edges.
    assert_eq!(placed.x + 11 + 1350, LAPTOP.x + 2880);
    assert_eq!(placed.y + 900, 1752);

    let placed = place_session_window(
        Some((-2000, 0)),
        1.5,
        Some(LEFT),
        (400.0, 300.0),
        SHADOW_FRAME,
    );
    // Its inner area starts at the work area's left and top edges, its
    // left frame outside.
    assert_eq!((placed.x + 11, placed.y), (LEFT.x, LEFT.y));
}

#[test]
fn away_from_the_edges_the_frame_changes_nothing() {
    let placed = place_session_window(
        Some((3600, 100)),
        1.5,
        Some(LAPTOP),
        (900.0, 600.0),
        SHADOW_FRAME,
    );
    assert_eq!((placed.x, placed.y), (3660, 160));
}

#[test]
fn it_is_never_smaller_than_the_least_a_session_window_has() {
    let placed = place_session_window(
        Some((100, 100)),
        1.5,
        Some(LAPTOP),
        (100.0, 50.0),
        Frame::default(),
    );
    assert_eq!((placed.width, placed.height), SESSION_WINDOW_MIN_SIZE);
}
