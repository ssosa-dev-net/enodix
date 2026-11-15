//! The user's preferences: the app's theme, the terminal's font and
//! scrollback, how copy and paste behave (multi-line paste confirmation
//! included), whether keywords are highlighted and whether the app checks for
//! updates. They live per user in the application data folder
//! (`preferences.yaml`), never in a workspace; the caller chooses that folder.
//!
//! Reading is forgiving: a value that is missing, of the wrong kind or out of
//! range falls back to (or is brought into) its allowed range on its own,
//! so a slip in a hand-edited file costs only that one value.

use std::fs;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

/// The preferences file's name inside the data folder.
const FILE_NAME: &str = "preferences.yaml";

pub const DEFAULT_FONT_FAMILY: &str = "Cascadia Mono";
pub const DEFAULT_FONT_SIZE: u16 = 14;
pub const MIN_FONT_SIZE: u16 = 8;
pub const MAX_FONT_SIZE: u16 = 36;
pub const DEFAULT_SCROLLBACK: u32 = 10_000;
/// Lines kept per terminal at most; each terminal holds its own.
pub const MAX_SCROLLBACK: u32 = 100_000;
/// The side view's width, in CSS pixels. (The right panel's width and
/// hidden flag of older files are ignored: host and folder details are a
/// sheet over the side view since #64.)
pub const DEFAULT_LEFT_PANEL_WIDTH: u16 = 288;
pub const MIN_PANEL_WIDTH: u16 = 200;
pub const MAX_PANEL_WIDTH: u16 = 800;

/// The app's colours.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Light or dark, as the operating system is set (and follows it).
    #[default]
    System,
    Light,
    Dark,
}

/// The side view the activity rail selected (the window's left side).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideView {
    /// The host tree.
    #[default]
    Hosts,
    Snippets,
    Jobs,
    /// Config backups.
    Backups,
    /// Credential profiles, named secrets and the vault.
    Credentials,
}

/// A step of the terminal font size (Ctrl+scroll, Ctrl+= / Ctrl+- / Ctrl+0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontZoom {
    In,
    Out,
    Reset,
}

/// What the user chose. Serialised with camelCase names for the interface;
/// the file uses snake_case (see `save`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub theme: Theme,
    /// The terminal's font, such as `Cascadia Mono` (the interface falls
    /// back to a monospace font when it is not installed).
    pub font_family: String,
    /// In CSS pixels, `MIN_FONT_SIZE..=MAX_FONT_SIZE`.
    pub font_size: u16,
    /// Lines each terminal keeps above the screen, `0..=MAX_SCROLLBACK`.
    pub scrollback: u32,
    /// Selecting text in a terminal copies it.
    pub copy_on_select: bool,
    /// A right click in a terminal pastes.
    pub right_click_paste: bool,
    /// Keyword highlighting in terminals, all rules at once (each rule can
    /// also be turned off on its own, in the workspace).
    pub highlighting: bool,
    /// A paste of more than one line asks first. Turning it off only
    /// applies to non-production hosts: see [`crate::paste`].
    pub confirm_multi_line_paste: bool,
    /// The app looks for a newer release once at start (see
    /// [`crate::update`]). Off, it makes no connection on its own.
    pub check_for_updates: bool,
    /// The side view the rail shows (`left_panel_width` is its width and
    /// `left_panel_hidden` whether it is folded away).
    pub side_view: SideView,
    /// The side view's width (pixels, `MIN_PANEL_WIDTH` to
    /// `MAX_PANEL_WIDTH`).
    pub left_panel_width: u16,
    /// The side view is folded away, for a wider terminal.
    pub left_panel_hidden: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            font_family: DEFAULT_FONT_FAMILY.into(),
            font_size: DEFAULT_FONT_SIZE,
            scrollback: DEFAULT_SCROLLBACK,
            copy_on_select: false,
            right_click_paste: false,
            highlighting: true,
            confirm_multi_line_paste: true,
            check_for_updates: true,
            side_view: SideView::Hosts,
            left_panel_width: DEFAULT_LEFT_PANEL_WIDTH,
            left_panel_hidden: false,
        }
    }
}

impl Preferences {
    /// Reads the preferences kept in `data_folder`. A missing or unreadable
    /// file gives the defaults; a bad value gives that value's default.
    pub fn load(data_folder: &Path) -> Self {
        let mapping = fs::read_to_string(data_folder.join(FILE_NAME))
            .ok()
            .and_then(|text| serde_norway::from_str::<Mapping>(&text).ok())
            .unwrap_or_default();
        let defaults = Self::default();
        Self {
            theme: read(&mapping, "theme").unwrap_or(defaults.theme),
            font_family: read(&mapping, "font_family").unwrap_or(defaults.font_family),
            font_size: read_number(&mapping, "font_size", u16::MAX).unwrap_or(defaults.font_size),
            scrollback: read_number(&mapping, "scrollback", u32::MAX)
                .unwrap_or(defaults.scrollback),
            copy_on_select: read(&mapping, "copy_on_select").unwrap_or(defaults.copy_on_select),
            right_click_paste: read(&mapping, "right_click_paste")
                .unwrap_or(defaults.right_click_paste),
            highlighting: read(&mapping, "highlighting").unwrap_or(defaults.highlighting),
            confirm_multi_line_paste: read(&mapping, "confirm_multi_line_paste")
                .unwrap_or(defaults.confirm_multi_line_paste),
            check_for_updates: read(&mapping, "check_for_updates")
                .unwrap_or(defaults.check_for_updates),
            side_view: read(&mapping, "side_view").unwrap_or(defaults.side_view),
            left_panel_width: read_number(&mapping, "left_panel_width", u16::MAX)
                .unwrap_or(defaults.left_panel_width),
            left_panel_hidden: read(&mapping, "left_panel_hidden")
                .unwrap_or(defaults.left_panel_hidden),
        }
        .normalized()
    }

    /// Writes the preferences (normalised) to `data_folder`, creating the
    /// folder if needed. The file is replaced whole, never left half written.
    pub fn save(&self, data_folder: &Path) -> io::Result<()> {
        let preferences = self.clone().normalized();
        let mut mapping = Mapping::new();
        let mut put = |key: &str, value: Value| {
            mapping.insert(Value::String(key.into()), value);
        };
        put(
            "theme",
            serde_norway::to_value(preferences.theme).map_err(io::Error::other)?,
        );
        put("font_family", Value::String(preferences.font_family));
        put("font_size", preferences.font_size.into());
        put("scrollback", preferences.scrollback.into());
        put("copy_on_select", preferences.copy_on_select.into());
        put("right_click_paste", preferences.right_click_paste.into());
        put("highlighting", preferences.highlighting.into());
        put(
            "confirm_multi_line_paste",
            preferences.confirm_multi_line_paste.into(),
        );
        put("check_for_updates", preferences.check_for_updates.into());
        put(
            "side_view",
            serde_norway::to_value(preferences.side_view).map_err(io::Error::other)?,
        );
        put("left_panel_width", preferences.left_panel_width.into());
        put("left_panel_hidden", preferences.left_panel_hidden.into());
        let text = serde_norway::to_string(&mapping).map_err(io::Error::other)?;
        fs::create_dir_all(data_folder)?;
        let temporary = data_folder.join(format!("{FILE_NAME}.tmp"));
        fs::write(&temporary, text)?;
        fs::rename(&temporary, data_folder.join(FILE_NAME))
    }

    /// The same preferences with every value in its allowed range: the font
    /// size and scrollback clamped, the font family trimmed (empty → the
    /// default).
    pub fn normalized(mut self) -> Self {
        self.font_size = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.scrollback = self.scrollback.min(MAX_SCROLLBACK);
        self.left_panel_width = self
            .left_panel_width
            .clamp(MIN_PANEL_WIDTH, MAX_PANEL_WIDTH);
        let family = self.font_family.trim();
        self.font_family = if family.is_empty() {
            DEFAULT_FONT_FAMILY.into()
        } else {
            family.into()
        };
        self
    }

    /// Steps the font size one pixel up or down (within range), or back to
    /// the default.
    pub fn zoom(&mut self, zoom: FontZoom) {
        self.font_size = match zoom {
            FontZoom::In => self.font_size.saturating_add(1),
            FontZoom::Out => self.font_size.saturating_sub(1),
            FontZoom::Reset => DEFAULT_FONT_SIZE,
        }
        .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    }
}

/// The value under `key`, if present and of the right kind.
fn read<T: DeserializeOwned>(mapping: &Mapping, key: &str) -> Option<T> {
    serde_norway::from_value(mapping.get(key)?.clone()).ok()
}

/// The whole number under `key`, at most `max` (a too-large number counts
/// as `max`, so that `normalized` brings it into range).
fn read_number<T: TryFrom<u64>>(mapping: &Mapping, key: &str, max: T) -> Option<T> {
    let number = mapping.get(key)?.as_u64()?;
    Some(T::try_from(number).unwrap_or(max))
}
