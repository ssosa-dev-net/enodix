//! The window's session layout: tabs in panes, and panes split side by side
//! or one above the other.
//!
//! A [`Layout`] only knows tab and pane ids; what each tab holds (a session,
//! its terminal) is the interface's. Every pane has its tabs in order and
//! shows at most one of them; a pane showing none shows the connect view
//! (the form for a new connection). One pane is active: new tabs open there,
//! and next/previous move through its tabs.
//!
//! Every change raises [`Layout::revision`]; an edit naming a tab or pane that
//! does not exist is refused and changes nothing.
//!
//! A tab holds a session or a view (#68): a [`View`] names which view it is
//! and what it is about. A view is open at most once: opening it again
//! shows its tab. View tabs move, split and close like session tabs, but
//! never join the multi-send group nor show a session. A session window's
//! layout ([`Layout::for_session_window`]) holds session tabs only.
//!
//! The app's windows each have a layout, kept together in [`Windows`] with
//! the one multi-send group that spans them (#83).

mod windows;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use windows::{LayoutMut, Member, WindowId, WindowView, Windows};

/// A tab, unique for the life of its [`Layout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(pub u64);

/// A pane, unique for the life of its [`Layout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(pub u64);

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A view shown in a tab: which view it is (a name the interface knows,
/// such as `preferences`) and what it is about (a host id, a run id), if it
/// is about something. Two tabs never show the same view.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct View {
    pub view: String,
    pub subject: Option<String>,
}

impl View {
    pub fn new(view: impl Into<String>, subject: Option<String>) -> Self {
        Self {
            view: view.into(),
            subject,
        }
    }
}

/// What a tab holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TabKind {
    /// A session's terminal ([`Layout::open_tab`]).
    Session,
    /// A view ([`Layout::open_view`]).
    View(View),
}

/// Where a split puts the new pane, and how a split lays out its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Side by side, the new pane on the right.
    Right,
    /// One above the other, the new pane below.
    Down,
}

/// A part of the layout: a pane, or panes split one way.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    Pane(Pane),
    Split(SplitNode),
}

/// Tabs shown in one area of the window.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pane {
    pub id: PaneId,
    /// In the order they are shown.
    pub tabs: Vec<TabId>,
    /// The tab shown; `None` shows the connect view.
    pub active: Option<TabId>,
}

/// Two or more parts laid out one way, in equal shares. Never holds a split
/// going the same way (that one's children join this one), nor fewer than
/// two children.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SplitNode {
    pub direction: Split,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("there is no tab {0}")]
    NoSuchTab(TabId),
    #[error("there is no pane {0}")]
    NoSuchPane(PaneId),
    #[error("the last pane cannot be closed")]
    LastPane,
    #[error("the tab {0} shows a view, not a session")]
    NotASession(TabId),
    #[error("a session window shows no view tabs")]
    NoViewTabs,
    #[error("there is no window {0}")]
    NoSuchWindow(WindowId),
    #[error("the main window cannot be closed")]
    MainWindow,
}

/// The tabs and panes of a window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    root: Node,
    active_pane: PaneId,
    /// What each tab holds.
    kinds: BTreeMap<TabId, TabKind>,
    revision: u64,
    #[serde(skip)]
    next_id: u64,
    /// Whether view tabs open here: not in a session window.
    #[serde(skip)]
    views: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    /// One empty pane, showing the connect view: the main window's.
    pub fn new() -> Self {
        let pane = PaneId(1);
        Self {
            root: Node::Pane(empty_pane(pane)),
            active_pane: pane,
            kinds: BTreeMap::new(),
            revision: 0,
            next_id: 2,
            views: true,
        }
    }

    /// One empty pane, for a session window: view tabs are refused.
    pub fn for_session_window() -> Self {
        Self {
            views: false,
            ..Self::new()
        }
    }

    /// Whether view tabs open in this layout (not a session window's).
    pub fn shows_views(&self) -> bool {
        self.views
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Raised by every change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn active_pane(&self) -> PaneId {
        self.active_pane
    }

    /// The tab shown in the active pane.
    pub fn active_tab(&self) -> Option<TabId> {
        self.pane(self.active_pane).and_then(|pane| pane.active)
    }

    /// Every pane, in reading order (left to right, top to bottom).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        each_pane(&self.root, &mut |pane| panes.push(pane.id));
        panes
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        let mut found = None;
        each_pane(&self.root, &mut |pane| {
            if pane.id == id {
                found = Some(pane);
            }
        });
        found
    }

    /// The pane holding `tab`.
    pub fn pane_of(&self, tab: TabId) -> Option<PaneId> {
        let mut found = None;
        each_pane(&self.root, &mut |pane| {
            if pane.tabs.contains(&tab) {
                found = Some(pane.id);
            }
        });
        found
    }

    /// The tab each pane shows, in reading order: the ones on screen.
    pub fn visible_tabs(&self) -> Vec<TabId> {
        let mut tabs = Vec::new();
        each_pane(&self.root, &mut |pane| tabs.extend(pane.active));
        tabs
    }

    /// What `tab` holds; `None` for a tab that does not exist.
    pub fn tab_kind(&self, tab: TabId) -> Option<&TabKind> {
        self.kinds.get(&tab)
    }

    /// Whether `tab` exists and shows a view.
    pub fn is_view(&self, tab: TabId) -> bool {
        matches!(self.tab_kind(tab), Some(TabKind::View(_)))
    }

    /// The tab showing `view`, if it is open.
    pub fn view_tab(&self, view: &View) -> Option<TabId> {
        self.kinds
            .iter()
            .find(|(_, kind)| matches!(kind, TabKind::View(open) if open == view))
            .map(|(&tab, _)| tab)
    }

    /// A new session tab at the end of the active pane, shown.
    pub fn open_tab(&mut self) -> TabId {
        self.open_new(TabKind::Session)
    }

    /// Shows `view`: its tab if it is open (that pane becomes active),
    /// else a new tab at the end of the active pane. Refused in a session
    /// window.
    pub fn open_view(&mut self, view: View) -> Result<TabId, LayoutError> {
        if !self.views {
            return Err(LayoutError::NoViewTabs);
        }
        if let Some(tab) = self.view_tab(&view) {
            self.activate(tab).expect("the view's tab exists");
            return Ok(tab);
        }
        Ok(self.open_new(TabKind::View(view)))
    }

    fn open_new(&mut self, kind: TabKind) -> TabId {
        let tab = TabId(self.new_id());
        self.kinds.insert(tab, kind);
        let pane = self.active_pane;
        let pane = self.pane_mut(pane).expect("the active pane exists");
        pane.tabs.push(tab);
        pane.active = Some(tab);
        self.changed();
        tab
    }

    /// A new tab right after `beside`, in its pane, shown.
    pub fn open_tab_beside(&mut self, beside: TabId) -> Result<TabId, LayoutError> {
        let pane_id = self.find_tab(beside)?;
        let tab = TabId(self.new_id());
        self.kinds.insert(tab, TabKind::Session);
        let pane = self.pane_mut(pane_id).expect("found above");
        let at = position(&pane.tabs, beside) + 1;
        pane.tabs.insert(at, tab);
        pane.active = Some(tab);
        self.active_pane = pane_id;
        self.changed();
        Ok(tab)
    }

    /// Shows `tab` and makes its pane the active one.
    pub fn activate(&mut self, tab: TabId) -> Result<(), LayoutError> {
        let pane_id = self.find_tab(tab)?;
        self.pane_mut(pane_id).expect("found above").active = Some(tab);
        self.active_pane = pane_id;
        self.changed();
        Ok(())
    }

    pub fn activate_pane(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        self.find_pane(pane)?;
        self.active_pane = pane;
        self.changed();
        Ok(())
    }

    /// Shows the connect view in `pane` (its tabs stay) and makes it active.
    pub fn show_connect_view(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        self.find_pane(pane)?;
        self.pane_mut(pane).expect("found above").active = None;
        self.active_pane = pane;
        self.changed();
        Ok(())
    }

    /// Shows the next tab of the active pane, going round; from the connect
    /// view, the first.
    pub fn next_tab(&mut self) {
        self.step(1);
    }

    /// Shows the previous tab of the active pane, going round; from the
    /// connect view, the last.
    pub fn previous_tab(&mut self) {
        self.step(-1);
    }

    /// Removes `tab`. Its pane then shows the tab to its right, else the one
    /// to its left; a pane left empty is removed, unless it is the last one
    /// (which then shows the connect view).
    pub fn close_tab(&mut self, tab: TabId) -> Result<(), LayoutError> {
        let pane_id = self.find_tab(tab)?;
        let pane = self.pane_mut(pane_id).expect("found above");
        take_tab(pane, tab);
        if pane.tabs.is_empty() {
            self.remove_pane_if_not_last(pane_id);
        }
        self.forget(tab);
        self.changed();
        Ok(())
    }

    /// Removes `tab` and shows the connect view in its pane, which stays and
    /// becomes active: for starting a new connection in its place.
    pub fn dismiss_tab(&mut self, tab: TabId) -> Result<(), LayoutError> {
        let pane_id = self.find_tab(tab)?;
        let pane = self.pane_mut(pane_id).expect("found above");
        take_tab(pane, tab);
        pane.active = None;
        self.active_pane = pane_id;
        self.forget(tab);
        self.changed();
        Ok(())
    }

    /// Moves `tab` by `offset` places within its pane, stopping at either end.
    pub fn move_tab_by(&mut self, tab: TabId, offset: isize) -> Result<(), LayoutError> {
        let pane_id = self.find_tab(tab)?;
        let pane = self.pane_mut(pane_id).expect("found above");
        let from = position(&pane.tabs, tab);
        let last = pane.tabs.len() - 1;
        let to = from.saturating_add_signed(offset).min(last);
        let moved = pane.tabs.remove(from);
        pane.tabs.insert(to, moved);
        self.changed();
        Ok(())
    }

    /// Moves `tab` into `pane` (its own or another) before the tab at `index`
    /// among that pane's other tabs (past the end: last), shows it there and
    /// makes that pane active. A pane it leaves empty is removed.
    pub fn move_tab(&mut self, tab: TabId, pane: PaneId, index: usize) -> Result<(), LayoutError> {
        let from = self.find_tab(tab)?;
        self.find_pane(pane)?;
        take_tab(self.pane_mut(from).expect("found above"), tab);
        let target = self.pane_mut(pane).expect("found above");
        let at = index.min(target.tabs.len());
        target.tabs.insert(at, tab);
        target.active = Some(tab);
        self.active_pane = pane;
        if from != pane && self.pane(from).is_some_and(|left| left.tabs.is_empty()) {
            self.remove_pane_if_not_last(from);
        }
        self.changed();
        Ok(())
    }

    /// Adds a pane beside `pane` (on its right, or below it), which becomes
    /// active. If `pane` has more than one tab, the one it shows moves into
    /// the new pane; otherwise the new pane is empty, for a new connection.
    pub fn split(&mut self, pane: PaneId, direction: Split) -> Result<PaneId, LayoutError> {
        self.find_pane(pane)?;
        let mut new = empty_pane(PaneId(self.new_id()));
        let source = self.pane_mut(pane).expect("found above");
        if let (true, Some(shown)) = (source.tabs.len() > 1, source.active) {
            take_tab(source, shown);
            new.tabs.push(shown);
            new.active = Some(shown);
        }
        let id = new.id;
        let mut new = Some(new);
        split_at(&mut self.root, pane, direction, &mut new);
        debug_assert!(new.is_none(), "the pane was found above");
        self.active_pane = id;
        self.changed();
        Ok(id)
    }

    /// Removes `pane`, giving its tabs to the pane before it in reading order
    /// (after its own), else the one after it (before its own). That pane
    /// keeps showing its tab, unless `pane` was the active one: then it shows
    /// the tab `pane` showed, and becomes active.
    pub fn close_pane(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        self.find_pane(pane)?;
        let Some((neighbour, before)) = self.neighbour(pane) else {
            return Err(LayoutError::LastPane);
        };
        let closing = self.pane(pane).expect("found above").clone();
        let was_active = self.active_pane == pane;
        remove_pane(&mut self.root, pane);
        let target = self.pane_mut(neighbour).expect("listed above");
        if before {
            target.tabs.extend(&closing.tabs);
        } else {
            target.tabs.splice(0..0, closing.tabs.iter().copied());
        }
        if was_active {
            if closing.active.is_some() {
                target.active = closing.active;
            }
            self.active_pane = neighbour;
        }
        self.changed();
        Ok(())
    }

    /// Drops what the layout knew about a tab that is gone.
    fn forget(&mut self, tab: TabId) {
        self.kinds.remove(&tab);
    }

    fn step(&mut self, by: isize) {
        let pane = self.active_pane;
        let pane = self.pane_mut(pane).expect("the active pane exists");
        if pane.tabs.is_empty() {
            return;
        }
        let count = pane.tabs.len() as isize;
        let next = match pane.active {
            Some(tab) => (position(&pane.tabs, tab) as isize + by).rem_euclid(count),
            None if by > 0 => 0,
            None => count - 1,
        };
        pane.active = Some(pane.tabs[next as usize]);
        self.changed();
    }

    /// Removes an empty `pane` unless it is the only one; the pane before it
    /// (else after it) becomes active if it was.
    fn remove_pane_if_not_last(&mut self, pane: PaneId) {
        let Some((neighbour, _)) = self.neighbour(pane) else {
            return;
        };
        remove_pane(&mut self.root, pane);
        if self.active_pane == pane {
            self.active_pane = neighbour;
        }
    }

    /// The pane that takes over from `pane` when it goes: the one before it
    /// in reading order (`true`), else the one after it (`false`); `None`
    /// when it is the only pane.
    fn neighbour(&self, pane: PaneId) -> Option<(PaneId, bool)> {
        let panes = self.panes();
        let at = panes.iter().position(|&id| id == pane)?;
        match at.checked_sub(1) {
            Some(previous) => Some((panes[previous], true)),
            None => panes.get(at + 1).map(|&next| (next, false)),
        }
    }

    fn find_tab(&self, tab: TabId) -> Result<PaneId, LayoutError> {
        self.pane_of(tab).ok_or(LayoutError::NoSuchTab(tab))
    }

    /// Whether `tab` exists and holds a session.
    fn find_session_tab(&self, tab: TabId) -> Result<(), LayoutError> {
        self.find_tab(tab)?;
        if self.is_view(tab) {
            return Err(LayoutError::NotASession(tab));
        }
        Ok(())
    }

    fn find_pane(&self, pane: PaneId) -> Result<(), LayoutError> {
        match self.pane(pane) {
            Some(_) => Ok(()),
            None => Err(LayoutError::NoSuchPane(pane)),
        }
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        pane_mut(&mut self.root, id)
    }

    fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn changed(&mut self) {
        self.revision += 1;
    }
}

fn empty_pane(id: PaneId) -> Pane {
    Pane {
        id,
        tabs: Vec::new(),
        active: None,
    }
}

fn position(tabs: &[TabId], tab: TabId) -> usize {
    tabs.iter()
        .position(|&t| t == tab)
        .expect("the tab is in this pane")
}

/// Removes `tab` from `pane`; if it was shown, shows its right neighbour,
/// else its left one.
fn take_tab(pane: &mut Pane, tab: TabId) {
    let at = position(&pane.tabs, tab);
    pane.tabs.remove(at);
    if pane.active == Some(tab) {
        pane.active = pane
            .tabs
            .get(at)
            .or_else(|| at.checked_sub(1).and_then(|left| pane.tabs.get(left)))
            .copied();
    }
}

fn each_pane<'a>(node: &'a Node, visit: &mut impl FnMut(&'a Pane)) {
    match node {
        Node::Pane(pane) => visit(pane),
        Node::Split(split) => {
            for child in &split.children {
                each_pane(child, visit);
            }
        }
    }
}

fn pane_mut(node: &mut Node, id: PaneId) -> Option<&mut Pane> {
    match node {
        Node::Pane(pane) => (pane.id == id).then_some(pane),
        Node::Split(split) => split
            .children
            .iter_mut()
            .find_map(|child| pane_mut(child, id)),
    }
}

/// Puts `new` beside the pane `id`: into its parent split if that goes the
/// same way, else into a new split in the pane's place.
fn split_at(node: &mut Node, id: PaneId, direction: Split, new: &mut Option<Pane>) {
    match node {
        Node::Pane(pane) if pane.id == id => {
            let old = std::mem::replace(node, Node::Pane(empty_pane(id)));
            let new = new.take().expect("put in once");
            *node = Node::Split(SplitNode {
                direction,
                children: vec![old, Node::Pane(new)],
            });
        }
        Node::Pane(_) => {}
        Node::Split(split) => {
            let here = split
                .children
                .iter()
                .position(|child| matches!(child, Node::Pane(pane) if pane.id == id))
                .filter(|_| split.direction == direction);
            if let Some(at) = here {
                let new = new.take().expect("put in once");
                split.children.insert(at + 1, Node::Pane(new));
                return;
            }
            for child in &mut split.children {
                if new.is_none() {
                    return;
                }
                split_at(child, id, direction, new);
            }
        }
    }
}

/// Removes the pane `id` (not the root) and tidies the splits around it.
fn remove_pane(root: &mut Node, id: PaneId) {
    fn remove(node: &mut Node, id: PaneId) {
        if let Node::Split(split) = node {
            split
                .children
                .retain(|child| !matches!(child, Node::Pane(pane) if pane.id == id));
            for child in &mut split.children {
                remove(child, id);
            }
        }
    }
    remove(root, id);
    tidy(root);
}

/// Replaces splits with one child by that child, and merges a split into its
/// parent when both go the same way.
fn tidy(node: &mut Node) {
    let Node::Split(split) = node else { return };
    for child in &mut split.children {
        tidy(child);
    }
    let direction = split.direction;
    let children = std::mem::take(&mut split.children);
    for child in children {
        match child {
            Node::Split(inner) if inner.direction == direction => {
                split.children.extend(inner.children);
            }
            other => split.children.push(other),
        }
    }
    if split.children.len() == 1 {
        let only = split.children.pop().expect("one child");
        *node = only;
    }
}
