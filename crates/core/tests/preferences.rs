//! The user's preferences (theme, terminal font, scrollback, copy and
//! paste): kept per user in the data folder, across restarts.

use std::fs;

use netterm_core::preferences::{
    FontZoom, Preferences, Theme, DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE,
};
use tempfile::TempDir;

#[test]
fn a_first_launch_follows_the_system_theme_with_the_default_terminal() {
    let data = TempDir::new().unwrap();

    let preferences = Preferences::load(data.path());

    assert_eq!(preferences, Preferences::default());
    assert_eq!(preferences.theme, Theme::System);
    assert_eq!(preferences.font_family, "Cascadia Mono");
    assert_eq!(preferences.font_size, DEFAULT_FONT_SIZE);
    assert_eq!(preferences.scrollback, 10_000);
    assert!(!preferences.copy_on_select);
    assert!(!preferences.right_click_paste);
}

#[test]
fn preferences_are_remembered_across_restarts() {
    let data = TempDir::new().unwrap();
    let chosen = Preferences {
        theme: Theme::Light,
        font_family: "JetBrains Mono".into(),
        font_size: 17,
        scrollback: 50_000,
        copy_on_select: true,
        right_click_paste: true,
    };

    chosen.save(data.path()).unwrap();

    assert_eq!(Preferences::load(data.path()), chosen);
}

#[test]
fn the_file_is_readable_yaml_in_the_data_folder() {
    let data = TempDir::new().unwrap();
    let folder = data.path().join("not/yet/there");
    Preferences {
        theme: Theme::Dark,
        ..Preferences::default()
    }
    .save(&folder)
    .unwrap();

    let text = fs::read_to_string(folder.join("preferences.yaml")).unwrap();
    assert!(text.contains("theme: dark"), "{text}");
    assert!(text.contains("font_size: 14"), "{text}");
    assert!(text.contains("copy_on_select: false"), "{text}");
}

#[test]
fn a_hand_edited_file_keeps_its_good_values_and_drops_the_bad_ones() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "theme: purple\nfont_family: Consolas\nfont_size: huge\nscrollback: 2000\ncopy_on_select: true\nunknown: 1\n",
    )
    .unwrap();

    let preferences = Preferences::load(data.path());

    assert_eq!(
        preferences,
        Preferences {
            font_family: "Consolas".into(),
            scrollback: 2000,
            copy_on_select: true,
            ..Preferences::default()
        }
    );
}

#[test]
fn an_unreadable_file_gives_the_defaults() {
    let data = TempDir::new().unwrap();
    fs::write(data.path().join("preferences.yaml"), "theme: [").unwrap();

    assert_eq!(Preferences::load(data.path()), Preferences::default());
}

#[test]
fn out_of_range_values_are_brought_into_range() {
    let preferences = Preferences {
        font_family: "   ".into(),
        font_size: 200,
        scrollback: 10_000_000,
        ..Preferences::default()
    }
    .normalized();

    assert_eq!(preferences.font_family, "Cascadia Mono");
    assert_eq!(preferences.font_size, MAX_FONT_SIZE);
    assert_eq!(preferences.scrollback, MAX_SCROLLBACK);
    let tiny = Preferences {
        font_family: "  Fira Code ".into(),
        font_size: 1,
        ..Preferences::default()
    }
    .normalized();
    assert_eq!(tiny.font_family, "Fira Code");
    assert_eq!(tiny.font_size, MIN_FONT_SIZE);
}

#[test]
fn out_of_range_values_in_the_file_are_brought_into_range() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "font_size: 3\nscrollback: 99999999\n",
    )
    .unwrap();

    let preferences = Preferences::load(data.path());

    assert_eq!(preferences.font_size, MIN_FONT_SIZE);
    assert_eq!(preferences.scrollback, MAX_SCROLLBACK);

    fs::write(
        data.path().join("preferences.yaml"),
        "font_size: 70000\nscrollback: 99999999999\n",
    )
    .unwrap();
    let preferences = Preferences::load(data.path());
    assert_eq!(preferences.font_size, MAX_FONT_SIZE);
    assert_eq!(preferences.scrollback, MAX_SCROLLBACK);
}

#[test]
fn saving_stores_the_normalized_values() {
    let data = TempDir::new().unwrap();
    Preferences {
        font_size: 99,
        ..Preferences::default()
    }
    .save(data.path())
    .unwrap();

    assert_eq!(Preferences::load(data.path()).font_size, MAX_FONT_SIZE);
}

#[test]
fn zooming_steps_the_font_size_within_its_range() {
    let mut preferences = Preferences::default();

    preferences.zoom(FontZoom::In);
    assert_eq!(preferences.font_size, DEFAULT_FONT_SIZE + 1);
    preferences.zoom(FontZoom::Out);
    preferences.zoom(FontZoom::Out);
    assert_eq!(preferences.font_size, DEFAULT_FONT_SIZE - 1);
    preferences.zoom(FontZoom::Reset);
    assert_eq!(preferences.font_size, DEFAULT_FONT_SIZE);

    preferences.font_size = MAX_FONT_SIZE;
    preferences.zoom(FontZoom::In);
    assert_eq!(preferences.font_size, MAX_FONT_SIZE);
    preferences.font_size = MIN_FONT_SIZE;
    preferences.zoom(FontZoom::Out);
    assert_eq!(preferences.font_size, MIN_FONT_SIZE);
}

#[test]
fn the_interface_sees_camel_case_names() {
    let json = serde_json::to_value(Preferences::default()).unwrap();

    assert_eq!(json["theme"], "system");
    assert_eq!(json["fontFamily"], "Cascadia Mono");
    assert_eq!(json["rightClickPaste"], false);
    let back: Preferences = serde_json::from_value(json).unwrap();
    assert_eq!(back, Preferences::default());
}
