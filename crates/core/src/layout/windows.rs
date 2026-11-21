//! The layouts of the app's windows, and the one multi-send group that
//! spans them (#83).
//!
//! [`Windows`] holds a [`Layout`] per window: the main window's (always
//! there, [`WindowId::MAIN`]) and one per session window, which shows no
//! view tabs. A tab id is unique within its window's layout only, so a
//! group member is named by window and tab ([`Member`]).
//!
//! The multi-send group belongs to the app (#35, #83): typing in a member
//! reaches every member's session, whichever window each is in
//! ([`Windows::input_targets`]). Nothing else fans out (#123): a paste
//! reaches the group only as a group paste, planned for the members at the
//! group's revision ([`Windows::group_paste_members`]), sent only if the
//! group is still as it was ([`Windows::group_paste_targets`]), each line
//! going to one member while it is one; a paste begun outside the group
//! stays with its tab. A production session joins only when the join says
//! the user confirmed it ([`Windows::join_group_confirmed`]). View tabs never join, new and duplicated
//! tabs start outside, a tab can join before its session is known (it
//! receives once [`Windows::attach_session`] names it), and closing a tab
//! (or its window) drops it; moving it to another window keeps it, in its
//! place (#87). Every change to the group raises every window's revision,
//! since what each window is sent includes its members and the group's
//! size, and the group's own revision ([`Windows::group_revision`]), by
//! which the shell knows to tell every window.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

use super::{Layout, LayoutError, PaneId, TabId};
use crate::paste::PasteSafety;
use crate::session::SessionId;

/// Where input in a terminal came from (#34), which decides which sessions
/// it reaches (#123); as the interface names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputOrigin {
    /// Keys the user typed: in a member, every member's.
    Typed,
    /// A paste (or snippet) into this session alone: never shared, even if
    /// its tab joined the group after the paste began.
    Pasted,
    /// One line of a group paste for this member: its own session, and only
    /// while it is a member.
    GroupPaste,
    /// The terminal's replies to the device: its own session only.
    Reply,
    /// Mouse reports: its own session only.
    Mouse,
}

/// Why a group paste was called off (#123): nothing of it was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GroupPasteRefused {
    #[error(
        "the tab pasted in is no longer in the multi-send group (it left, or the group was ended), so nothing was pasted"
    )]
    NotAMember,
    #[error(
        "the multi-send group changed before the paste was sent, so nothing was pasted; paste again to send it to the group as it is now"
    )]
    Changed,
}

/// Who a paste in a member goes to (#123): every member whose session is
/// known, the one pasted in first, and the group's revision they were
/// read at, which the paste must still find when it is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupPasteMembers {
    pub revision: u64,
    pub members: Vec<(Member, SessionId)>,
}

/// A window of the app that has a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowId(pub u64);

impl WindowId {
    /// The main window: there from the start, never closed.
    pub const MAIN: Self = Self(0);
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A member of the multi-send group: a tab of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Member {
    pub window: WindowId,
    pub tab: TabId,
}

/// The layouts of the app's windows and the multi-send group.
#[derive(Debug)]
pub struct Windows {
    layouts: BTreeMap<WindowId, Layout>,
    next_window: u64,
    group: Group,
    /// What a session tab moved to a session window showed (#84), until
    /// the window's page takes it.
    snapshots: HashMap<Member, String>,
    /// The session whose terminal was used last, in any window (#86).
    last_used: Option<SessionId>,
    /// Raised by every change to the group's members (#87).
    group_revision: u64,
}

/// The terminal used last (#86): where snippets from the main window's
/// Snippets view go, whichever window it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Used {
    pub window: WindowId,
    pub tab: TabId,
    pub session: SessionId,
}

/// A session a window takes (#84): its tab there, and what the window that
/// gave it up showed, as escape sequences; `None` when that is lost (the
/// page showing it went away, or reloaded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Taken {
    pub tab: TabId,
    pub session: SessionId,
    pub snapshot: Option<String>,
}

/// The multi-send group, and the session each tab shows (for routing).
#[derive(Debug, Default)]
struct Group {
    /// In the order they joined.
    members: Vec<Member>,
    /// The session each session tab shows, once known.
    sessions: HashMap<Member, SessionId>,
}

impl Group {
    /// Drops what the group knew about the tabs that are `gone`; whether a
    /// member went.
    fn forget(&mut self, gone: impl Fn(&Member) -> bool) -> bool {
        self.sessions.retain(|member, _| !gone(member));
        let before = self.members.len();
        self.members.retain(|member| !gone(member));
        self.members.len() != before
    }
}

/// What a window's interface is sent: its layout, which of its tabs are
/// in the multi-send group (in the order they joined), and how many
/// members the group has in every window (#87).
#[derive(Clone, Debug, Serialize)]
pub struct WindowView {
    #[serde(flatten)]
    pub layout: Layout,
    pub group: Vec<TabId>,
    #[serde(rename = "groupSize")]
    pub group_size: usize,
    /// The group's revision ([`Windows::group_revision`]): a group paste's
    /// question asked at an older one is out of date (#123).
    #[serde(rename = "groupRevision")]
    pub group_revision: u64,
}

/// A window's layout, open for editing. Once it is dropped, the
/// multi-send group forgets the tabs the edit closed (raising every
/// window's revision if one was a member).
pub struct LayoutMut<'a> {
    window: WindowId,
    windows: &'a mut Windows,
}

impl Deref for LayoutMut<'_> {
    type Target = Layout;

    fn deref(&self) -> &Layout {
        &self.windows.layouts[&self.window]
    }
}

impl DerefMut for LayoutMut<'_> {
    fn deref_mut(&mut self) -> &mut Layout {
        self.windows
            .layouts
            .get_mut(&self.window)
            .expect("the window exists while its layout is edited")
    }
}

impl Drop for LayoutMut<'_> {
    fn drop(&mut self) {
        let window = self.window;
        let Windows { layouts, group, .. } = &mut *self.windows;
        let layout = &layouts[&window];
        let closed =
            |member: &Member| member.window == window && layout.tab_kind(member.tab).is_none();
        if group.forget(closed) {
            self.windows.group_changed();
        }
    }
}

impl Default for Windows {
    fn default() -> Self {
        Self::new()
    }
}

impl Windows {
    /// The main window, with an empty layout; no group.
    pub fn new() -> Self {
        Self {
            layouts: BTreeMap::from([(WindowId::MAIN, Layout::new())]),
            next_window: WindowId::MAIN.0 + 1,
            group: Group::default(),
            group_revision: 0,
            snapshots: HashMap::new(),
            last_used: None,
        }
    }

    /// Every window, the main one first, then in the order they opened.
    pub fn windows(&self) -> Vec<WindowId> {
        self.layouts.keys().copied().collect()
    }

    /// A new session window, with an empty layout that shows no views.
    pub fn open_session_window(&mut self) -> WindowId {
        let window = WindowId(self.next_window);
        self.next_window += 1;
        self.layouts.insert(window, Layout::for_session_window());
        window
    }

    /// Forgets a session window: its layout and its members go. The main
    /// window is refused.
    pub fn close_window(&mut self, window: WindowId) -> Result<(), LayoutError> {
        if window == WindowId::MAIN {
            return Err(LayoutError::MainWindow);
        }
        self.layouts
            .remove(&window)
            .ok_or(LayoutError::NoSuchWindow(window))?;
        self.forget_window(window);
        Ok(())
    }

    pub fn layout(&self, window: WindowId) -> Option<&Layout> {
        self.layouts.get(&window)
    }

    /// `window`'s layout, to edit: see [`LayoutMut`].
    pub fn layout_mut(&mut self, window: WindowId) -> Option<LayoutMut<'_>> {
        if !self.layouts.contains_key(&window) {
            return None;
        }
        Some(LayoutMut {
            window,
            windows: self,
        })
    }

    /// Starts `window`'s layout afresh (a session window's stays one); its
    /// tabs leave the group.
    pub fn reset(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let layout = self
            .layouts
            .get_mut(&window)
            .ok_or(LayoutError::NoSuchWindow(window))?;
        *layout = if layout.shows_views() {
            Layout::new()
        } else {
            Layout::for_session_window()
        };
        self.forget_window(window);
        Ok(())
    }

    /// What `window`'s interface is sent.
    pub fn view(&self, window: WindowId) -> Option<WindowView> {
        Some(WindowView {
            layout: self.layout(window)?.clone(),
            group: self.window_group(window),
            group_size: self.group.members.len(),
            group_revision: self.group_revision,
        })
    }

    /// The multi-send group's members, in the order they joined.
    pub fn group(&self) -> &[Member] {
        &self.group.members
    }

    /// Raised by every change to the group's members: a tab joining or
    /// leaving, its tab or window closing, the group ending. A member
    /// moving to another window keeps it (#87).
    pub fn group_revision(&self) -> u64 {
        self.group_revision
    }

    /// Where a paste in `window`'s tab `tab` goes when it is a member
    /// (#87): every member whose session is known, with it, the one pasted
    /// in first, then the others in the order they joined, whichever
    /// window each is in. Empty when the tab is not a member (it pastes
    /// into its own session only).
    pub fn paste_members(&self, window: WindowId, tab: TabId) -> Vec<(Member, SessionId)> {
        let from = Member { window, tab };
        if !self.group.members.contains(&from) {
            return Vec::new();
        }
        std::iter::once(from)
            .chain(self.group.members.iter().copied().filter(|&m| m != from))
            .filter_map(|member| Some((member, *self.group.sessions.get(&member)?)))
            .collect()
    }

    /// The member that shows `session` now, wherever it moved (#87): where
    /// its lines of a group paste go. `None` if no member shows it.
    pub fn member_showing(&self, session: SessionId) -> Option<Member> {
        self.group
            .members
            .iter()
            .copied()
            .find(|member| self.group.sessions.get(member) == Some(&session))
    }

    /// Who a paste in `window`'s tab `tab` goes to (#123): as
    /// [`Windows::paste_members`], with the group's revision now, which the
    /// paste must still find when it is sent. Refused when the tab is not a
    /// member (it left, or the group ended): the paste is called off.
    pub fn group_paste_members(
        &self,
        window: WindowId,
        tab: TabId,
    ) -> Result<GroupPasteMembers, GroupPasteRefused> {
        if !self.in_group(window, tab) {
            return Err(GroupPasteRefused::NotAMember);
        }
        Ok(GroupPasteMembers {
            revision: self.group_revision,
            members: self.paste_members(window, tab),
        })
    }

    /// Where each of `sessions` gets its lines of a group paste from
    /// `window`'s tab `tab`, planned (and asked about) at the group's
    /// `revision` (#123): the member showing it now, in the same order. The
    /// paste goes to the members as they are when it is sent, which are the
    /// ones its question showed: refused whole, so that nothing is sent,
    /// if the tab is no longer a member or the group changed since (a tab
    /// joined or left, a member closed, the group ended). A member that
    /// moved to another window meanwhile is still one.
    pub fn group_paste_targets(
        &self,
        window: WindowId,
        tab: TabId,
        revision: u64,
        sessions: &[SessionId],
    ) -> Result<Vec<Member>, GroupPasteRefused> {
        if !self.in_group(window, tab) {
            return Err(GroupPasteRefused::NotAMember);
        }
        if revision != self.group_revision {
            return Err(GroupPasteRefused::Changed);
        }
        sessions
            .iter()
            .map(|&session| {
                self.member_showing(session)
                    .ok_or(GroupPasteRefused::Changed)
            })
            .collect()
    }

    /// The members that are tabs of `window`, in the order they joined.
    pub fn window_group(&self, window: WindowId) -> Vec<TabId> {
        self.group
            .members
            .iter()
            .filter(|member| member.window == window)
            .map(|member| member.tab)
            .collect()
    }

    /// Whether `window`'s tab `tab` is in the multi-send group.
    pub fn in_group(&self, window: WindowId, tab: TabId) -> bool {
        self.group.members.contains(&Member { window, tab })
    }

    /// Adds `window`'s tab `tab` to the group (nothing changes if it is in
    /// it). A view tab is refused.
    pub fn join_group(&mut self, window: WindowId, tab: TabId) -> Result<(), LayoutError> {
        self.existing(window)?.find_session_tab(tab)?;
        if !self.in_group(window, tab) {
            self.group.members.push(Member { window, tab });
            self.group_changed();
        }
        Ok(())
    }

    /// Adds every session tab of `window`'s `pane` that is not a member yet
    /// to the group, in the pane's order, and returns them (nothing
    /// changes if there are none).
    pub fn join_pane(&mut self, window: WindowId, pane: PaneId) -> Result<Vec<TabId>, LayoutError> {
        let layout = self.existing(window)?;
        let joining: Vec<TabId> = layout
            .pane(pane)
            .ok_or(LayoutError::NoSuchPane(pane))?
            .tabs
            .iter()
            .copied()
            .filter(|&tab| !layout.is_view(tab) && !self.in_group(window, tab))
            .collect();
        if !joining.is_empty() {
            self.group
                .members
                .extend(joining.iter().map(|&tab| Member { window, tab }));
            self.group_changed();
        }
        Ok(joining)
    }

    /// Adds `window`'s tab `tab` to the group as [`Windows::join_group`]
    /// does, if the join may be made (#123): a production session (by
    /// `safety`, its session's paste safety now) joins only when the join
    /// says the user `confirmed` it, and so does a tab whose session is not
    /// known yet (its safety cannot be told). Refused, changing nothing,
    /// otherwise. The shell joins only this way.
    pub fn join_group_confirmed(
        &mut self,
        window: WindowId,
        tab: TabId,
        confirmed: bool,
        safety: impl Fn(SessionId) -> PasteSafety,
    ) -> Result<(), LayoutError> {
        self.existing(window)?.find_session_tab(tab)?;
        if !self.in_group(window, tab) && !confirmed {
            self.refuse_unconfirmed(window, &[tab], &safety)?;
        }
        self.join_group(window, tab)
    }

    /// Adds every session tab of `window`'s `pane` that is not a member yet
    /// to the group as [`Windows::join_pane`] does, if the join may be
    /// made: as for [`Windows::join_group_confirmed`], one tab that needs
    /// the user's yes refuses the whole join when it was not given.
    pub fn join_pane_confirmed(
        &mut self,
        window: WindowId,
        pane: PaneId,
        confirmed: bool,
        safety: impl Fn(SessionId) -> PasteSafety,
    ) -> Result<Vec<TabId>, LayoutError> {
        let layout = self.existing(window)?;
        let joining: Vec<TabId> = layout
            .pane(pane)
            .ok_or(LayoutError::NoSuchPane(pane))?
            .tabs
            .iter()
            .copied()
            .filter(|&tab| !layout.is_view(tab) && !self.in_group(window, tab))
            .collect();
        if !confirmed {
            self.refuse_unconfirmed(window, &joining, &safety)?;
        }
        self.join_pane(window, pane)
    }

    /// Refuses a join of `tabs` that was not confirmed if one of them needs
    /// it: a session not known yet, or a production one.
    fn refuse_unconfirmed(
        &self,
        window: WindowId,
        tabs: &[TabId],
        safety: &impl Fn(SessionId) -> PasteSafety,
    ) -> Result<(), LayoutError> {
        let mut production = Vec::new();
        for &tab in tabs {
            let session = self
                .session_of(window, tab)
                .ok_or(LayoutError::JoinBeforeSession(tab))?;
            if safety(session).confirms_joining() {
                production.push(tab);
            }
        }
        if production.is_empty() {
            Ok(())
        } else {
            Err(LayoutError::JoinNotConfirmed(production))
        }
    }

    /// Takes `window`'s tab `tab` out of the group (nothing changes if it
    /// is not in it).
    pub fn leave_group(&mut self, window: WindowId, tab: TabId) -> Result<(), LayoutError> {
        self.existing(window)?.find_tab(tab)?;
        if self.in_group(window, tab) {
            self.group
                .members
                .retain(|&member| member != Member { window, tab });
            self.group_changed();
        }
        Ok(())
    }

    /// Ends the group: no tab of any window is in it any more.
    pub fn end_group(&mut self) {
        if !self.group.members.is_empty() {
            self.group.members.clear();
            self.group_changed();
        }
    }

    /// Records that `window`'s tab `tab` shows `session` (a reconnect keeps
    /// the same session). Not part of what the interface is sent, so no
    /// revision changes. A view tab is refused.
    pub fn attach_session(
        &mut self,
        window: WindowId,
        tab: TabId,
        session: SessionId,
    ) -> Result<(), LayoutError> {
        self.existing(window)?.find_session_tab(tab)?;
        self.group.sessions.insert(Member { window, tab }, session);
        Ok(())
    }

    /// The sessions that input from `origin` in `session` goes to (#123).
    /// Typed: every member's session (in the order they joined), whichever
    /// window it is in, if `session`'s tab is a member, else only `session`
    /// itself; members whose session is not known yet are left out. A
    /// group paste's line: `session` itself while a member shows it, else
    /// none (it left, closed, or the group ended mid-paste). Anything else
    /// (a paste of its own, the terminal's replies, mouse reports): only
    /// `session`, whatever the group.
    pub fn input_targets(&self, session: SessionId, origin: InputOrigin) -> Vec<SessionId> {
        let sessions = &self.group.sessions;
        let member = self
            .group
            .members
            .iter()
            .any(|member| sessions.get(member) == Some(&session));
        match origin {
            InputOrigin::Typed if member => {}
            InputOrigin::GroupPaste if member => return vec![session],
            InputOrigin::GroupPaste => return Vec::new(),
            _ => return vec![session],
        }
        self.group
            .members
            .iter()
            .filter_map(|member| sessions.get(member).copied())
            .collect()
    }

    /// The session tabs of `window` whose session is known, with it, in
    /// the layout's order (panes in reading order).
    pub fn session_tabs(&self, window: WindowId) -> Vec<(TabId, SessionId)> {
        let Some(layout) = self.layout(window) else {
            return Vec::new();
        };
        layout
            .panes()
            .into_iter()
            .filter_map(|pane| layout.pane(pane))
            .flat_map(|pane| pane.tabs.iter().copied())
            .filter_map(|tab| {
                let session = self.group.sessions.get(&Member { window, tab })?;
                Some((tab, *session))
            })
            .collect()
    }

    /// Each session window, in the order they opened, with the session its
    /// active tab shows (by which it is named, #85); `None` while that is
    /// not known.
    pub fn session_windows(&self) -> Vec<(WindowId, Option<SessionId>)> {
        self.layouts
            .iter()
            .filter(|(&window, _)| window != WindowId::MAIN)
            .map(|(&window, layout)| {
                let session = layout
                    .active_tab()
                    .and_then(|tab| self.group.sessions.get(&Member { window, tab }))
                    .copied();
                (window, session)
            })
            .collect()
    }

    /// The session `window`'s tab `tab` shows, once known.
    pub fn session_of(&self, window: WindowId, tab: TabId) -> Option<SessionId> {
        self.group.sessions.get(&Member { window, tab }).copied()
    }

    /// Records that the terminal of `window`'s session tab `tab` was used
    /// (it took the keyboard): it is the one used last (#86). A tab whose
    /// session is not known yet changes nothing; a view tab or an unknown
    /// window is refused.
    pub fn use_tab(&mut self, window: WindowId, tab: TabId) -> Result<(), LayoutError> {
        self.existing(window)?.find_session_tab(tab)?;
        if let Some(session) = self.session_of(window, tab) {
            self.last_used = Some(session);
        }
        Ok(())
    }

    /// The terminal used last, where its session is shown now (it follows
    /// the session from window to window); `None` once no tab shows it.
    pub fn last_used(&self) -> Option<Used> {
        let used = self.last_used?;
        self.group
            .sessions
            .iter()
            .find(|(_, &session)| session == used)
            .map(|(member, &session)| Used {
                window: member.window,
                tab: member.tab,
                session,
            })
    }

    /// Moves the session tab `tab` of the window `from` to the window `to`
    /// (#84), where it becomes a new tab at the end of the active pane,
    /// shown; returns its id there. Its session goes with it, and a member
    /// stays in the group, in its place (#87; the group's revision stays).
    /// A session window left without a tab closes (#85). A view tab, the same window or an unknown one is
    /// refused, changing nothing.
    pub fn move_session(
        &mut self,
        from: WindowId,
        tab: TabId,
        to: WindowId,
    ) -> Result<TabId, LayoutError> {
        self.existing(from)?.find_session_tab(tab)?;
        self.existing(to)?;
        if from == to {
            return Err(LayoutError::SameWindow);
        }
        let session = self
            .group
            .sessions
            .get(&Member { window: from, tab })
            .copied();
        let moved = self.layouts.get_mut(&to).expect("checked above").open_tab();
        if let Some(session) = session {
            self.group.sessions.insert(
                Member {
                    window: to,
                    tab: moved,
                },
                session,
            );
        }
        let old = Member { window: from, tab };
        let new = Member {
            window: to,
            tab: moved,
        };
        if let Some(place) = self.group.members.iter_mut().find(|m| **m == old) {
            *place = new;
        }
        // Dropping the edit forgets the closed tab's session.
        self.layout_mut(from)
            .expect("checked above")
            .close_tab(tab)?;
        if from != WindowId::MAIN && self.emptied(from) {
            self.close_window(from)?;
        }
        Ok(moved)
    }

    /// Keeps `snapshot` (what the window giving it up showed) for the
    /// session tab `tab` of the session window `window`, until its page
    /// takes the session (#84).
    pub fn keep_snapshot(&mut self, window: WindowId, tab: TabId, snapshot: String) {
        self.snapshots.insert(Member { window, tab }, snapshot);
    }

    /// The sessions `window` shows (its session tabs, in reading order),
    /// for its page to take as it loads, each with the snapshot kept for
    /// it, used up; empty while it shows none (nothing has moved there
    /// yet). A page taking them again (it reloaded) gets no snapshots:
    /// what it showed is lost.
    pub fn take_sessions(&mut self, window: WindowId) -> Vec<Taken> {
        self.session_tabs(window)
            .into_iter()
            .map(|(tab, session)| Taken {
                tab,
                session,
                snapshot: self.snapshots.remove(&Member { window, tab }),
            })
            .collect()
    }

    /// The session tab `tab` that moved into `window` while its page was
    /// open (#85), with its snapshot, used up as `take_sessions` does;
    /// `None` if the window does not show it.
    pub fn take_tab(&mut self, window: WindowId, tab: TabId) -> Option<Taken> {
        let member = Member { window, tab };
        let session = *self.group.sessions.get(&member)?;
        Some(Taken {
            tab,
            session,
            snapshot: self.snapshots.remove(&member),
        })
    }

    /// The session window `window` went away without docking back (#84):
    /// each of its sessions moves to the main window, which shows them in
    /// new tabs (with the snapshot its page never took, if any), and the
    /// window closes. Returns them, in order.
    pub fn hand_to_main(&mut self, window: WindowId) -> Result<Vec<Taken>, LayoutError> {
        if window == WindowId::MAIN {
            return Err(LayoutError::MainWindow);
        }
        self.existing(window)?;
        let mut handed = Vec::new();
        for (tab, session) in self.session_tabs(window) {
            let snapshot = self.snapshots.remove(&Member { window, tab });
            let tab = self.move_session(window, tab, WindowId::MAIN)?;
            handed.push(Taken {
                tab,
                session,
                snapshot,
            });
        }
        // Its last tab moving out closed it, unless a tab whose session was
        // not known yet (a duplicate opening) was left.
        if self.layouts.contains_key(&window) {
            self.close_window(window)?;
        }
        Ok(handed)
    }

    /// Every session window's session tabs whose session is known, window
    /// by window in the order they opened, each in reading order (#88):
    /// what closing the main window or switching workspace asks about,
    /// besides the main window's own.
    pub fn session_window_sessions(&self) -> Vec<(Member, SessionId)> {
        self.layouts
            .keys()
            .filter(|&&window| window != WindowId::MAIN)
            .flat_map(|&window| {
                self.session_tabs(window)
                    .into_iter()
                    .map(move |(tab, session)| (Member { window, tab }, session))
            })
            .collect()
    }

    /// Closes, in the session windows, every tab showing one of `sessions`
    /// (#88: their workspace was switched away from); a member leaves the
    /// group. A session window left without a tab closes: those are
    /// returned, in order. The main window's tabs are left alone (its page
    /// closes its own).
    pub fn close_sessions(&mut self, sessions: &[SessionId]) -> Vec<WindowId> {
        let closing: Vec<Member> = self
            .session_window_sessions()
            .into_iter()
            .filter(|(_, session)| sessions.contains(session))
            .map(|(member, _)| member)
            .collect();
        let mut closed = Vec::new();
        for member in closing {
            let Some(mut layout) = self.layout_mut(member.window) else {
                continue;
            };
            // Dropping the edit forgets the closed tab's session.
            if layout.close_tab(member.tab).is_err() {
                continue;
            }
            drop(layout);
            self.snapshots.remove(&member);
            if self.emptied(member.window) && self.close_window(member.window).is_ok() {
                closed.push(member.window);
            }
        }
        closed
    }

    /// Whether every pane of `window` is without a tab.
    fn emptied(&self, window: WindowId) -> bool {
        self.layout(window).is_some_and(|layout| {
            layout
                .panes()
                .iter()
                .all(|&pane| layout.pane(pane).is_some_and(|pane| pane.tabs.is_empty()))
        })
    }

    /// Drops every member and session of `window`.
    fn forget_window(&mut self, window: WindowId) {
        self.snapshots.retain(|member, _| member.window != window);
        if self.group.forget(|member| member.window == window) {
            self.group_changed();
        }
    }

    fn existing(&self, window: WindowId) -> Result<&Layout, LayoutError> {
        self.layout(window).ok_or(LayoutError::NoSuchWindow(window))
    }

    /// Every window is sent its members and the group's size, so every
    /// revision rises, and the group's own.
    fn group_changed(&mut self) {
        self.group_revision += 1;
        for layout in self.layouts.values_mut() {
            layout.changed();
        }
    }
}
