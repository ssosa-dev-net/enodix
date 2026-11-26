//! The user's preferences (theme, terminal font, scrollback, copy and
//! paste): kept per user in the data folder, across restarts.

use std::fs;

use enodix_core::preferences::{
    FontZoom, Preferences, SideView, Theme, DEFAULT_FONT_SIZE, DEFAULT_LEFT_PANEL_WIDTH,
    MAX_FONT_SIZE, MAX_PANEL_WIDTH, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_PANEL_WIDTH,
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
fn the_side_view_starts_at_its_default_width_and_shown() {
    let preferences = Preferences::default();
    assert_eq!(preferences.left_panel_width, DEFAULT_LEFT_PANEL_WIDTH);
    assert!(!preferences.left_panel_hidden);
}

#[test]
fn the_side_view_width_and_folding_are_remembered_across_restarts() {
    let data = TempDir::new().unwrap();
    let chosen = Preferences {
        left_panel_width: 420,
        left_panel_hidden: true,
        ..Preferences::default()
    };

    chosen.save(data.path()).unwrap();

    assert_eq!(Preferences::load(data.path()), chosen);
}

#[test]
fn the_side_view_width_is_kept_within_its_range() {
    let narrow = Preferences {
        left_panel_width: 10,
        ..Preferences::default()
    }
    .normalized();
    assert_eq!(narrow.left_panel_width, MIN_PANEL_WIDTH);

    let wide = Preferences {
        left_panel_width: 5000,
        ..Preferences::default()
    }
    .normalized();
    assert_eq!(wide.left_panel_width, MAX_PANEL_WIDTH);

    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "left_panel_width: 99999999999\nleft_panel_hidden: maybe\n",
    )
    .unwrap();
    let loaded = Preferences::load(data.path());
    assert_eq!(loaded.left_panel_width, MAX_PANEL_WIDTH);
    assert!(!loaded.left_panel_hidden);
}

#[test]
fn the_side_view_starts_on_hosts() {
    let data = TempDir::new().unwrap();

    assert_eq!(Preferences::load(data.path()).side_view, SideView::Hosts);
}

#[test]
fn the_side_view_its_width_and_folding_are_remembered_across_restarts() {
    let data = TempDir::new().unwrap();
    let chosen = Preferences {
        side_view: SideView::Jobs,
        left_panel_width: 360,
        left_panel_hidden: true,
        ..Preferences::default()
    };

    chosen.save(data.path()).unwrap();

    let text = fs::read_to_string(data.path().join("preferences.yaml")).unwrap();
    assert!(text.contains("side_view: jobs"), "{text}");
    assert_eq!(Preferences::load(data.path()), chosen);
    for view in [
        SideView::Hosts,
        SideView::Snippets,
        SideView::Jobs,
        SideView::Backups,
        SideView::Credentials,
    ] {
        let chosen = Preferences {
            side_view: view,
            ..Preferences::default()
        };
        chosen.save(data.path()).unwrap();
        assert_eq!(Preferences::load(data.path()).side_view, view);
    }
}

#[test]
fn a_preferences_file_from_before_the_side_views_keeps_its_values() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "theme: dark\nfont_size: 16\nleft_panel_width: 333\nright_panel_width: 250\nleft_panel_hidden: true\nright_panel_hidden: true\n",
    )
    .unwrap();

    let loaded = Preferences::load(data.path());

    assert_eq!(loaded.side_view, SideView::Hosts);
    assert_eq!(loaded.theme, Theme::Dark);
    assert_eq!(loaded.font_size, 16);
    assert_eq!(loaded.left_panel_width, 333);
    assert!(loaded.left_panel_hidden);
}

#[test]
fn an_unknown_side_view_falls_back_to_hosts() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "side_view: dashboards\nfont_size: 15\n",
    )
    .unwrap();

    let loaded = Preferences::load(data.path());

    assert_eq!(loaded.side_view, SideView::Hosts);
    assert_eq!(loaded.font_size, 15);
}

#[test]
fn the_interface_sees_the_side_view_by_name() {
    let json = serde_json::to_value(Preferences {
        side_view: SideView::Backups,
        ..Preferences::default()
    })
    .unwrap();

    assert_eq!(json["sideView"], "backups");
}

#[test]
fn an_old_file_with_right_panel_values_loads_and_ignores_them() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "font_size: 17\nleft_panel_width: 345\nright_panel_width: 420\nleft_panel_hidden: false\nright_panel_hidden: true\n",
    )
    .unwrap();

    let loaded = Preferences::load(data.path());

    assert_eq!(loaded.font_size, 17);
    assert_eq!(loaded.left_panel_width, 345);
    assert!(!loaded.left_panel_hidden);
    let json = serde_json::to_value(&loaded).unwrap();
    assert!(json.get("rightPanelWidth").is_none(), "{json}");
    assert!(json.get("rightPanelHidden").is_none(), "{json}");

    loaded.save(data.path()).unwrap();
    let text = fs::read_to_string(data.path().join("preferences.yaml")).unwrap();
    assert!(!text.contains("right_panel"), "{text}");
    assert_eq!(Preferences::load(data.path()), loaded);
}

#[test]
fn the_interface_ignores_right_panel_values_it_sends() {
    let mut json = serde_json::to_value(Preferences::default()).unwrap();
    json["rightPanelWidth"] = 500.into();
    json["rightPanelHidden"] = true.into();

    let back: Preferences = serde_json::from_value(json).unwrap();

    assert_eq!(back, Preferences::default());
}

#[test]
fn a_file_written_by_0_2_0_carries_every_choice_over() {
    // Every key 0.2.0 wrote, none at its default (the redesign, #61, must
    // keep them all but the right panel's, which it retired).
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("preferences.yaml"),
        "theme: light
font_family: JetBrains Mono
font_size: 18
scrollback: 2500
copy_on_select: true
right_click_paste: true
highlighting: false
confirm_multi_line_paste: false
check_for_updates: false
left_panel_width: 512
right_panel_width: 300
left_panel_hidden: true
right_panel_hidden: true
",
    )
    .unwrap();

    let loaded = Preferences::load(data.path());

    assert_eq!(
        loaded,
        Preferences {
            theme: Theme::Light,
            font_family: "JetBrains Mono".into(),
            font_size: 18,
            scrollback: 2500,
            copy_on_select: true,
            right_click_paste: true,
            highlighting: false,
            confirm_multi_line_paste: false,
            check_for_updates: false,
            left_panel_width: 512,
            left_panel_hidden: true,
            side_view: SideView::Hosts,
            local_shell: String::new(),
        }
    );
}

#[test]
fn the_local_shell_is_remembered_and_defaults_to_the_computers_first() {
    let data = TempDir::new().unwrap();
    assert_eq!(Preferences::default().local_shell, "");

    let chosen = Preferences {
        local_shell: "  wsl:Ubuntu ".into(),
        ..Preferences::default()
    };
    chosen.save(data.path()).unwrap();

    assert_eq!(Preferences::load(data.path()).local_shell, "wsl:Ubuntu");
}
