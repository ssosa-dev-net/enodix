//! The update check: the app asks the release server for the latest
//! release's manifest (`latest.json`, the Tauri updater's format) and tells
//! the user when a newer version exists. An installed copy offers to install
//! it (only when the user says so); a portable copy only notifies. With the
//! check turned off, the app connects nowhere on its own.
//!
//! Every test talks to an HTTP server inside the test process.

mod support;

use std::fs;
use std::time::{Duration, Instant};

use netterm_core::preferences::Preferences;
use netterm_core::update::{
    InstallKind, InstallRefused, UpdateAction, UpdateChecker, UpdateSource, UpdateSourceError,
    UpdateStatus, INSTALLED_MARKER, PORTABLE_MARKER,
};
use support::http::{Reply, TestHttpServer};
use tempfile::TempDir;

/// A manifest as the release script writes it, announcing `version`.
fn manifest(version: &str) -> String {
    format!(
        r#"{{
  "version": "{version}",
  "notes": "Faster serial ports.",
  "pub_date": "2026-11-01T10:00:00Z",
  "page": "https://example.invalid/netterm/releases/tag/v{version}",
  "platforms": {{
    "windows-x86_64": {{
      "signature": "c2lnbmF0dXJl",
      "url": "https://example.invalid/netterm/releases/download/v{version}/netterm_{version}_x64-setup.exe"
    }}
  }}
}}"#
    )
}

fn checker(server: &TestHttpServer, current: &str, kind: InstallKind) -> UpdateChecker {
    let source = UpdateSource::new(&server.url("/latest.json")).unwrap();
    UpdateChecker::new(Some(source), current, kind).unwrap()
}

fn enabled() -> Preferences {
    Preferences::default()
}

fn disabled() -> Preferences {
    Preferences {
        check_for_updates: false,
        ..Preferences::default()
    }
}

// --- Installed and portable copies ------------------------------------------

#[tokio::test]
async fn an_installed_copy_offers_to_install_a_newer_version() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;

    let status = checker(&server, "0.1.0", InstallKind::Installed)
        .check_now()
        .await;

    let UpdateStatus::Available(offer) = status else {
        panic!("expected an offer, got {status:?}");
    };
    assert_eq!(offer.version, "0.2.0");
    assert_eq!(offer.current, "0.1.0");
    assert_eq!(offer.action, UpdateAction::Install);
    assert_eq!(offer.notes.as_deref(), Some("Faster serial ports."));
    assert_eq!(offer.published.as_deref(), Some("2026-11-01T10:00:00Z"));
    assert_eq!(
        offer.page.as_deref(),
        Some("https://example.invalid/netterm/releases/tag/v0.2.0")
    );
    assert_eq!(server.requests(), ["GET /latest.json HTTP/1.1"]);
}

#[tokio::test]
async fn a_portable_copy_only_notifies() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;

    let status = checker(&server, "0.1.0", InstallKind::Portable)
        .check_now()
        .await;

    let UpdateStatus::Available(offer) = status else {
        panic!("expected a notification, got {status:?}");
    };
    assert_eq!(offer.action, UpdateAction::Notify);
    assert_eq!(offer.version, "0.2.0");
}

// --- Installing: only what was offered, only in an installed copy ------------

#[tokio::test]
async fn an_installed_copy_installs_only_the_version_it_offered() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;
    let checker = checker(&server, "0.1.0", InstallKind::Installed);
    assert_eq!(
        checker.installable(true),
        Err(InstallRefused::NothingOffered)
    );

    checker.check_now().await;

    let offer = checker.installable(true).unwrap();
    assert_eq!(offer.version, "0.2.0");
    assert!(offer.is_version("0.2.0"));
    assert!(offer.is_version("v0.2.0"));
    assert!(!offer.is_version("0.2.1"));
    assert!(!offer.is_version("not a version"));
}

#[tokio::test]
async fn a_portable_copy_never_installs() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;
    let checker = checker(&server, "0.1.0", InstallKind::Portable);

    checker.check_now().await;

    assert_eq!(checker.installable(true), Err(InstallRefused::NotifyOnly));
}

#[tokio::test]
async fn a_build_without_the_updater_cannot_install() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;
    let checker = checker(&server, "0.1.0", InstallKind::Installed);

    checker.check_now().await;

    assert_eq!(checker.installable(false), Err(InstallRefused::NoUpdater));
}

#[tokio::test]
async fn being_up_to_date_withdraws_an_earlier_offer_and_a_failure_keeps_it() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;
    let checker = checker(&server, "0.1.0", InstallKind::Installed);
    checker.check_now().await;

    server.reply(Reply::Status(500, Vec::new()));
    assert!(matches!(
        checker.check_now().await,
        UpdateStatus::Failed { .. }
    ));
    assert!(checker.installable(true).is_ok());

    server.reply(Reply::Status(200, manifest("0.1.0").into_bytes()));
    checker.check_now().await;
    assert_eq!(
        checker.installable(true),
        Err(InstallRefused::NothingOffered)
    );
}

// --- Installed or portable --------------------------------------------------

#[test]
fn a_copy_beside_the_installers_uninstaller_is_installed() {
    let folder = TempDir::new().unwrap();
    fs::write(folder.path().join(INSTALLED_MARKER), "").unwrap();

    assert_eq!(InstallKind::detect(folder.path()), InstallKind::Installed);
}

#[test]
fn any_other_copy_only_notifies() {
    // A copied program, a development build, the portable zip.
    let folder = TempDir::new().unwrap();
    assert_eq!(InstallKind::detect(folder.path()), InstallKind::Portable);

    fs::write(folder.path().join(PORTABLE_MARKER), "portable").unwrap();
    assert_eq!(InstallKind::detect(folder.path()), InstallKind::Portable);

    // The portable marker wins, even beside an uninstaller.
    fs::write(folder.path().join(INSTALLED_MARKER), "").unwrap();
    assert_eq!(InstallKind::detect(folder.path()), InstallKind::Portable);
}

#[test]
fn a_folder_named_like_the_uninstaller_does_not_make_a_copy_installed() {
    let folder = TempDir::new().unwrap();
    fs::create_dir(folder.path().join(INSTALLED_MARKER)).unwrap();

    assert_eq!(InstallKind::detect(folder.path()), InstallKind::Portable);
}

// --- Versions ------------------------------------------------------------------

#[tokio::test]
async fn the_same_version_is_up_to_date() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;

    let status = checker(&server, "0.2.0", InstallKind::Installed)
        .check_now()
        .await;

    assert_eq!(
        status,
        UpdateStatus::UpToDate {
            current: "0.2.0".into()
        }
    );
}

#[tokio::test]
async fn an_older_release_is_never_offered() {
    let server = TestHttpServer::serving(&manifest("0.1.9")).await;

    let status = checker(&server, "0.2.0", InstallKind::Installed)
        .check_now()
        .await;

    assert_eq!(
        status,
        UpdateStatus::UpToDate {
            current: "0.2.0".into()
        }
    );
}

#[tokio::test]
async fn versions_compare_as_numbers_and_may_start_with_v() {
    let server = TestHttpServer::serving(&manifest("v0.10.0")).await;

    let status = checker(&server, "0.9.3", InstallKind::Installed)
        .check_now()
        .await;

    let UpdateStatus::Available(offer) = status else {
        panic!("expected an offer, got {status:?}");
    };
    assert_eq!(offer.version, "0.10.0");
}

#[tokio::test]
async fn a_pre_release_is_older_than_its_release() {
    let server = TestHttpServer::serving(&manifest("1.0.0-beta.2")).await;

    let status = checker(&server, "1.0.0", InstallKind::Installed)
        .check_now()
        .await;

    assert!(
        matches!(status, UpdateStatus::UpToDate { .. }),
        "{status:?}"
    );
}

#[test]
fn the_running_version_must_be_a_version() {
    assert!(UpdateChecker::new(None, "one point oh", InstallKind::Installed).is_err());
}

// --- Turning the check off ----------------------------------------------------

#[tokio::test]
async fn with_the_check_off_the_app_connects_nowhere_on_its_own() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;

    let status = checker(&server, "0.1.0", InstallKind::Installed)
        .automatic(&disabled())
        .await;

    assert_eq!(status, UpdateStatus::Disabled);
    // Give a stray request time to arrive.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn with_the_check_on_the_app_asks_once_at_start() {
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;

    let status = checker(&server, "0.1.0", InstallKind::Installed)
        .automatic(&enabled())
        .await;

    assert!(matches!(status, UpdateStatus::Available(_)), "{status:?}");
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn the_user_can_still_check_with_the_automatic_check_off() {
    // "Check now" is a connection the user starts.
    let server = TestHttpServer::serving(&manifest("0.2.0")).await;
    let checker = checker(&server, "0.1.0", InstallKind::Installed);

    assert_eq!(checker.automatic(&disabled()).await, UpdateStatus::Disabled);
    assert!(matches!(
        checker.check_now().await,
        UpdateStatus::Available(_)
    ));
    assert_eq!(server.connections(), 1);
}

#[test]
fn the_check_is_on_by_default_and_remembered_when_turned_off() {
    let data = TempDir::new().unwrap();
    assert!(Preferences::load(data.path()).check_for_updates);

    disabled().save(data.path()).unwrap();

    let text = fs::read_to_string(data.path().join("preferences.yaml")).unwrap();
    assert!(text.contains("check_for_updates: false"), "{text}");
    assert!(!Preferences::load(data.path()).check_for_updates);
    let json = serde_json::to_value(Preferences::default()).unwrap();
    assert_eq!(json["checkForUpdates"], true);
}

#[tokio::test]
async fn a_build_without_a_release_server_never_checks() {
    let checker = UpdateChecker::new(None, "0.1.0", InstallKind::Installed).unwrap();

    assert_eq!(
        checker.automatic(&enabled()).await,
        UpdateStatus::NotConfigured
    );
    assert_eq!(checker.check_now().await, UpdateStatus::NotConfigured);
}

// --- Failures -------------------------------------------------------------------

#[tokio::test]
async fn a_missing_manifest_is_reported() {
    let server = TestHttpServer::start(Reply::Status(404, b"not found".to_vec())).await;

    let status = checker(&server, "0.1.0", InstallKind::Installed)
        .check_now()
        .await;

    let UpdateStatus::Failed { reason } = status else {
        panic!("expected a failure, got {status:?}");
    };
    assert!(reason.contains("404"), "{reason}");
}

#[tokio::test]
async fn a_manifest_that_is_not_one_is_reported() {
    for body in ["<html>maintenance</html>", r#"{"notes": "no version"}"#] {
        let server = TestHttpServer::serving(body).await;

        let status = checker(&server, "0.1.0", InstallKind::Installed)
            .check_now()
            .await;

        assert!(matches!(status, UpdateStatus::Failed { .. }), "{status:?}");
    }
}

#[tokio::test]
async fn a_manifest_with_a_bad_version_is_reported() {
    let server = TestHttpServer::serving(&manifest("latest")).await;

    let status = checker(&server, "0.1.0", InstallKind::Installed)
        .check_now()
        .await;

    let UpdateStatus::Failed { reason } = status else {
        panic!("expected a failure, got {status:?}");
    };
    assert!(reason.contains("latest"), "{reason}");
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let server = TestHttpServer::start(Reply::Silence).await;
    let checker =
        checker(&server, "0.1.0", InstallKind::Installed).with_timeout(Duration::from_millis(300));
    let started = Instant::now();

    let status = checker.check_now().await;

    assert!(matches!(status, UpdateStatus::Failed { .. }), "{status:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn an_unreachable_server_is_reported() {
    // A port nothing listens on.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let source = UpdateSource::new(&format!("http://127.0.0.1:{port}/latest.json")).unwrap();
    let checker = UpdateChecker::new(Some(source), "0.1.0", InstallKind::Installed).unwrap();

    assert!(matches!(
        checker.check_now().await,
        UpdateStatus::Failed { .. }
    ));
}

#[tokio::test]
async fn an_oversized_manifest_is_refused() {
    let big = format!(
        r#"{{"version": "0.2.0", "notes": "{}"}}"#,
        "x".repeat(2 * 1024 * 1024)
    );
    let server = TestHttpServer::serving(&big).await;

    let status = checker(&server, "0.1.0", InstallKind::Installed)
        .check_now()
        .await;

    assert!(matches!(status, UpdateStatus::Failed { .. }), "{status:?}");
}

#[tokio::test]
async fn a_release_page_that_is_not_a_web_address_is_left_out() {
    let body = r#"{"version": "0.2.0", "page": "file:///C:/Windows/System32/calc.exe"}"#;
    let server = TestHttpServer::serving(body).await;

    let status = checker(&server, "0.1.0", InstallKind::Portable)
        .check_now()
        .await;

    let UpdateStatus::Available(offer) = status else {
        panic!("expected a notification, got {status:?}");
    };
    assert_eq!(offer.page, None);
    assert_eq!(offer.notes, None);
}

// --- The release server's address ------------------------------------------------

#[test]
fn the_release_server_must_be_reached_over_https() {
    assert!(UpdateSource::new("https://releases.example.invalid/latest.json").is_ok());
    assert!(matches!(
        UpdateSource::new("http://releases.example.invalid/latest.json"),
        Err(UpdateSourceError::NotHttps { .. })
    ));
    assert!(matches!(
        UpdateSource::new("ftp://releases.example.invalid/latest.json"),
        Err(UpdateSourceError::NotHttps { .. })
    ));
    assert!(matches!(
        UpdateSource::new("not a url"),
        Err(UpdateSourceError::NotAUrl)
    ));
}

#[test]
fn an_override_replaces_the_built_in_release_server_and_a_bad_one_means_no_check() {
    let built_in = Some("https://releases.example.invalid/latest.json");
    let local = "http://127.0.0.1:9/latest.json";

    let chosen = |over: Option<&str>, built: Option<&str>| {
        UpdateSource::choose(over, built).map(|source| source.url().to_string())
    };

    assert_eq!(chosen(None, built_in).as_deref(), built_in);
    assert_eq!(chosen(Some(local), built_in).as_deref(), Some(local));
    // Never a fall-back to somewhere else.
    assert_eq!(
        chosen(Some("http://192.168.0.1/latest.json"), built_in),
        None
    );
    assert_eq!(chosen(None, Some("http://releases.example.invalid/")), None);
    assert_eq!(chosen(None, None), None);
}

#[test]
fn plain_http_is_allowed_only_on_this_computer() {
    // For tests and the smoke test's local release server.
    assert!(UpdateSource::new("http://127.0.0.1:8080/latest.json").is_ok());
    assert!(UpdateSource::new("http://[::1]:8080/latest.json").is_ok());
    assert!(UpdateSource::new("http://localhost:8080/latest.json").is_ok());
    assert!(UpdateSource::new("http://192.168.0.1/latest.json").is_err());
}

#[test]
fn the_interface_sees_tagged_camel_case_statuses() {
    let json = serde_json::to_value(UpdateStatus::UpToDate {
        current: "0.1.0".into(),
    })
    .unwrap();
    assert_eq!(json["kind"], "upToDate");
    assert_eq!(json["current"], "0.1.0");
    assert_eq!(
        serde_json::to_value(UpdateStatus::Disabled).unwrap()["kind"],
        "disabled"
    );
    assert_eq!(
        serde_json::to_value(InstallKind::Portable).unwrap(),
        "portable"
    );
}
