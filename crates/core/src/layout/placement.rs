//! Where a new session window opens (#90), in physical pixels.
//!
//! A logical position does not name one monitor when monitors have
//! different display scaling: the windowing library resolves it by trying
//! each monitor's scale in turn. So the position is worked out in pixels,
//! from the calling window's outer position and its monitor's work area.
//! The size stays logical (the shell applies it at the scale of the
//! monitor the window opens on), so the terminal keeps its columns and
//! rows.

/// How far a new session window opens from the calling window's corner,
/// right and down, in logical pixels.
const OPEN_OFFSET: f64 = 40.0;

/// The smallest inner size of a session window, in logical pixels.
pub const SESSION_WINDOW_MIN_SIZE: (f64, f64) = (420.0, 320.0);

/// A rectangle in physical pixels, such as a monitor's work area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How far a window's inner area lies right of and below its outer
/// position, in physical pixels: its frame, visible or not. What it is
/// depends on the platform, so the shell measures it (on Windows a window
/// with a shadow and no title bar has an invisible border left, right and
/// below). The inner area is what is seen, so it is what must fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub left: u32,
    pub top: u32,
}

/// Where a new session window opens: its outer position in physical
/// pixels and its inner size in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: f64,
    pub height: f64,
}

/// Places a session window `size` (logical, at least
/// [`SESSION_WINDOW_MIN_SIZE`]) 40 logical pixels right and down
/// of `from` (the calling window's outer position, physical), on a monitor
/// at `scale`. With the monitor's `work_area` known, the window is shrunk
/// so that its inner area fits it and moved so that its inner area is
/// inside it, its `frame` left out; without `from`, it opens near the work
/// area's corner.
pub fn place_session_window(
    from: Option<(i32, i32)>,
    scale: f64,
    work_area: Option<PhysicalRect>,
    (mut width, mut height): (f64, f64),
    frame: Frame,
) -> Placement {
    width = width.max(SESSION_WINDOW_MIN_SIZE.0);
    height = height.max(SESSION_WINDOW_MIN_SIZE.1);
    let offset = (OPEN_OFFSET * scale).round() as i64;
    let (from_x, from_y) = from
        .or_else(|| work_area.map(|area| (area.x, area.y)))
        .unwrap_or((0, 0));
    let mut x = i64::from(from_x) + offset;
    let mut y = i64::from(from_y) + offset;
    if let Some(area) = work_area {
        width = width.min(f64::from(area.width) / scale);
        height = height.min(f64::from(area.height) / scale);
        let across = (width * scale).round() as i64;
        let down = (height * scale).round() as i64;
        x = fit(x + i64::from(frame.left), across, area.x, area.width) - i64::from(frame.left);
        y = fit(y + i64::from(frame.top), down, area.y, area.height) - i64::from(frame.top);
    }
    Placement {
        x: clamp_i32(x),
        y: clamp_i32(y),
        width,
        height,
    }
}

/// `at`, moved so that `length` from it lies inside the span from `start`.
fn fit(at: i64, length: i64, start: i32, span: u32) -> i64 {
    let start = i64::from(start);
    at.min(start + i64::from(span) - length).max(start)
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
