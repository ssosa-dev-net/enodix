//! The user's preferences (theme, terminal font, scrollback, copy and
//! paste): kept per user in the data folder, across restarts.

use std::fs;

use netterm_core::preferences::{
    FontZoom, Preferences, Theme, DEFAULT_FONT_SIZE, DEFAULT_LEFT_PANEL_WIDTH,
    DEFAULT_RIGHT_PANEL_WIDTH, MAX_FONT_SIZE, MAX_PANEL_WIDTH, MAX_SCROLLBACK, MIN_FONT_SIZE,
    MIN_PANEL_WIDTH,
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
    assert!(preferences.highlighting);
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
        highlighting: false,
        confirm_multi_line_paste: false,
        check_for_updates: false,
        ..Preferences::default()
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
    assert!(text.contains("highlighting: true"), "{text}");
}

#[test]
fn highlighting_can_be_turned_off_for_every_terminal() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "highlighting: false
",
    )
    .unwrap();
    assert!(!Preferences::load(data.path()).highlighting);

    // A value that is not a switch keeps highlighting on.
    fs::write(
        data.path().join("preferences.yaml"),
        "highlighting: maybe
",
    )
    .unwrap();
    assert!(Preferences::load(data.path()).highlighting);
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

#[test]
fn side_panels_start_at_their_default_widths_and_shown() {
    let preferences = Preferences::default();
    assert_eq!(preferences.left_panel_width, DEFAULT_LEFT_PANEL_WIDTH);
    assert_eq!(preferences.right_panel_width, DEFAULT_RIGHT_PANEL_WIDTH);
    assert!(!preferences.left_panel_hidden);
    assert!(!preferences.right_panel_hidden);
}

#[test]
fn side_panel_widths_and_visibility_are_remembered_across_restarts() {
    let data = TempDir::new().unwrap();
    let chosen = Preferences {
        left_panel_width: 420,
        right_panel_width: 380,
        left_panel_hidden: true,
        right_panel_hidden: false,
        ..Preferences::default()
    };

    chosen.save(data.path()).unwrap();

    assert_eq!(Preferences::load(data.path()), chosen);
}

#[test]
fn side_panel_widths_are_kept_within_their_range() {
    let narrow = Preferences {
        left_panel_width: 10,
        right_panel_width: 0,
        ..Preferences::default()
    }
    .normalized();
    assert_eq!(narrow.left_panel_width, MIN_PANEL_WIDTH);
    assert_eq!(narrow.right_panel_width, MIN_PANEL_WIDTH);

    let wide = Preferences {
        left_panel_width: 5000,
        right_panel_width: 900,
        ..Preferences::default()
    }
    .normalized();
    assert_eq!(wide.left_panel_width, MAX_PANEL_WIDTH);
    assert_eq!(wide.right_panel_width, MAX_PANEL_WIDTH);

    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "left_panel_width: 99999999999\nright_panel_width: wide\nleft_panel_hidden: maybe\n",
    )
    .unwrap();
    let loaded = Preferences::load(data.path());
    assert_eq!(loaded.left_panel_width, MAX_PANEL_WIDTH);
    assert_eq!(loaded.right_panel_width, DEFAULT_RIGHT_PANEL_WIDTH);
    assert!(!loaded.left_panel_hidden);
}
