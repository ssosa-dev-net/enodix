//! A session window that died without giving its sessions back (#84, #89):
//! destroyed, or its page crashed while the window stayed open.

use super::{LayoutError, Taken, WindowId, Windows};
use crate::session::SessionRelays;

/// Takes the session window `window`'s sessions away from it: each session
/// tab moves to the main window ([`Windows::hand_to_main`], the snapshot
/// its page never took or `None`, its output so far lost) and whoever held
/// the session is let go of ([`SessionRelays::let_go`]), so its
/// unacknowledged output no longer holds flow control back and the main
/// window can attach. The window is closed in `windows`; the caller closes
/// the real one too, so no page is left showing a session it no longer
/// owns. The main window, or one already gone, is refused.
pub fn take_over_dead_window(
    windows: &mut Windows,
    relays: &SessionRelays,
    window: WindowId,
) -> Result<Vec<Taken>, LayoutError> {
    let handed = windows.hand_to_main(window)?;
    for taken in &handed {
        relays.let_go(taken.session);
    }
    Ok(handed)
}
