//! Web links: named URL templates set on hosts and folders, merged down the
//! tree, filled from the host's fields, and opened only if they are http or
//! https addresses.

use std::fs;

use enodix_core::template::Variables;
use enodix_core::workspace::{
    web_url, HostId, HostWebLink, Origin, ResolvedHost, WebLink, WebLinkError, Workspace,
};
use tempfile::TempDir;

/// A workspace with Panorama on the root, a GUI link and a controller on
/// the Firewalls folder, and two firewalls: `fw1` uses what it inherits,
/// `fw2` replaces the GUI link and adds its own.
fn workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("_folder.yaml"),
        "web_links:\n  Panorama: https://panorama.example.net/devices?name={{name}}\n",
    )
    .unwrap();
    fs::create_dir(root.join("Firewalls")).unwrap();
    fs::write(
        root.join("Firewalls/_folder.yaml"),
        "web_links:\n  GUI: https://{{address}}/\n  Controller: http://{{custom.controller}}:8080/\n",
    )
    .unwrap();
    fs::write(
        root.join("Firewalls/fw1.yaml"),
        "address: 10.0.0.1\ncustom:\n  controller: ctl-mad\n",
    )
    .unwrap();
    fs::write(
        root.join("Firewalls/fw2.yaml"),
        "address: 10.0.0.2\nweb_links:\n  GUI: https://{{address}}:{{custom.gui_port}}/login\n  Grafana: https://grafana/d/{{site}}\ncustom:\n  gui_port: 4443\nsite: MAD1\n",
    )
    .unwrap();
    let workspace = Workspace::open(root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

fn resolved(workspace: &Workspace, id: &str) -> ResolvedHost {
    workspace.resolve_host(&HostId::new(id)).unwrap()
}

fn firewalls() -> Origin {
    Origin::Folder {
        path: "Firewalls".into(),
    }
}

fn root() -> Origin {
    Origin::Folder { path: "".into() }
}

/// Each link's name, origin and the URL it opens (or why it does not).
fn summary(links: Vec<HostWebLink>) -> Vec<(String, Origin, Result<String, String>)> {
    links
        .into_iter()
        .map(|link| {
            (
                link.name,
                link.origin,
                link.url.map_err(|error| error.to_string()),
            )
        })
        .collect()
}

fn ok(url: &str) -> Result<String, String> {
    Ok(url.to_string())
}

// --- Merging and filling ---

#[test]
fn a_host_offers_its_own_links_and_the_ones_it_inherits_filled_from_its_fields() {
    let (_temp, workspace) = workspace();
    assert_eq!(
        summary(resolved(&workspace, "Firewalls/fw1.yaml").web_links()),
        [
            (
                "Panorama".to_string(),
                root(),
                ok("https://panorama.example.net/devices?name=fw1")
            ),
            ("GUI".to_string(), firewalls(), ok("https://10.0.0.1/")),
            (
                "Controller".to_string(),
                firewalls(),
                ok("http://ctl-mad:8080/")
            ),
        ]
    );
}

#[test]
fn a_hosts_link_replaces_an_inherited_one_of_the_same_name_in_its_place() {
    let (_temp, workspace) = workspace();
    let links = summary(resolved(&workspace, "Firewalls/fw2.yaml").web_links());
    let names: Vec<&str> = links.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(names, ["Panorama", "GUI", "Controller", "Grafana"]);
    assert_eq!(
        links[1],
        (
            "GUI".to_string(),
            Origin::Host,
            ok("https://10.0.0.2:4443/login")
        )
    );
    assert_eq!(
        links[3],
        (
            "Grafana".to_string(),
            Origin::Host,
            ok("https://grafana/d/MAD1")
        )
    );
}

#[test]
fn a_link_using_a_field_the_host_has_no_value_for_is_not_opened() {
    let (_temp, workspace) = workspace();
    // fw2 has no `controller` custom field.
    let host = resolved(&workspace, "Firewalls/fw2.yaml");
    let controller = host
        .web_links()
        .into_iter()
        .find(|link| link.name == "Controller")
        .unwrap();
    assert_eq!(controller.template, "http://{{custom.controller}}:8080/");
    let Err(WebLinkError::Missing(missing)) = &controller.url else {
        panic!("expected a missing variable, got {:?}", controller.url);
    };
    assert_eq!(missing.names, ["custom.controller"]);
    assert_eq!(
        controller.url.unwrap_err().to_string(),
        "the host has no value for custom.controller"
    );
}

#[test]
fn a_link_is_opened_by_name() {
    let (_temp, workspace) = workspace();
    let host = resolved(&workspace, "Firewalls/fw2.yaml");
    assert_eq!(
        host.web_link_url("GUI").unwrap(),
        "https://10.0.0.2:4443/login"
    );
    assert!(matches!(
        host.web_link_url("Controller"),
        Err(WebLinkError::Missing(_))
    ));
    let unknown = host.web_link_url("Nope").unwrap_err();
    assert!(matches!(&unknown, WebLinkError::NoSuchLink(name) if name == "Nope"));
    assert_eq!(unknown.to_string(), "the host has no web link named Nope");
}

#[test]
fn a_link_fills_with_the_snippet_templating() {
    let link = WebLink::new("GUI", "https://{{ Address }}/{{a}}");
    let values: Variables = [("address", "10.0.0.9"), ("a", "{{address}}")]
        .into_iter()
        .collect();
    // Filled as snippets are: names ignoring case, values never expanded
    // again (here the braces are then percent-encoded in the URL).
    assert_eq!(link.fill(&values).unwrap(), "https://10.0.0.9/{{address}}");
    assert_eq!(
        link.open_url(&values).unwrap(),
        "https://10.0.0.9/%7B%7Baddress%7D%7D"
    );
}

#[test]
fn a_link_reaches_the_interface_with_its_url_or_why_it_is_refused() {
    let (_temp, workspace) = workspace();
    let links = resolved(&workspace, "Firewalls/fw2.yaml").web_links();
    assert_eq!(
        serde_json::to_value(&links[1]).unwrap(),
        serde_json::json!({
            "name": "GUI",
            "template": "https://{{address}}:{{custom.gui_port}}/login",
            "origin": { "kind": "host" },
            "url": "https://10.0.0.2:4443/login",
            "refused": null,
        })
    );
    assert_eq!(
        serde_json::to_value(&links[2]).unwrap(),
        serde_json::json!({
            "name": "Controller",
            "template": "http://{{custom.controller}}:8080/",
            "origin": { "kind": "folder", "path": "Firewalls" },
            "url": null,
            "refused": "the host has no value for custom.controller",
        })
    );
}

// --- Which URLs are opened ---

#[test]
fn http_and_https_urls_are_opened() {
    for (text, opened) in [
        ("https://10.0.0.1/", "https://10.0.0.1/"),
        ("http://10.0.0.1:8080/ui", "http://10.0.0.1:8080/ui"),
        ("HTTPS://FW1.example.net", "https://fw1.example.net/"),
        ("  https://fw1/  ", "https://fw1/"),
        ("https://[2001:db8::1]:8443/", "https://[2001:db8::1]:8443/"),
        ("https://fw1/a b?q=x y", "https://fw1/a%20b?q=x%20y"),
    ] {
        assert_eq!(web_url(text).unwrap(), opened, "{text:?}");
    }
}

#[test]
fn other_schemes_are_refused_with_a_message() {
    for (text, scheme) in [
        ("file:///C:/Windows/System32/calc.exe", "file"),
        ("javascript:alert(1)", "javascript"),
        ("ssh://10.0.0.1", "ssh"),
        ("ftp://10.0.0.1/", "ftp"),
        ("mailto:noc@example.net", "mailto"),
        ("data:text/html,<b>x</b>", "data"),
        ("C:\\Windows\\notepad.exe", "c"),
    ] {
        let error = web_url(text).unwrap_err();
        assert!(
            matches!(&error, WebLinkError::NotWeb { scheme: s, .. } if s == scheme),
            "{text:?}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!(
                "only http and https links are opened, not {scheme}: {}",
                text.trim()
            ),
        );
    }
}

#[test]
fn text_that_is_not_a_url_is_refused_with_a_message() {
    for text in [
        "",
        "10.0.0.1",
        "fw1/login",
        "https://",
        "http://[::1/",
        "https://a b/",
    ] {
        let error = web_url(text).unwrap_err();
        assert!(
            matches!(error, WebLinkError::NotAUrl { .. }),
            "{text:?}: {error:?}"
        );
        assert!(
            error
                .to_string()
                .starts_with(&format!("\"{}\" is not a web address: ", text.trim())),
            "{error}"
        );
    }
}

#[test]
fn a_url_holding_a_password_is_refused_without_showing_it() {
    let error = web_url("https://admin:s3cret@fw1/").unwrap_err();
    assert_eq!(error, WebLinkError::HoldsPassword);
    assert_eq!(
        error.to_string(),
        "a link may not hold a password: the app keeps no web logins"
    );
    // A user name alone is no secret.
    assert_eq!(web_url("https://admin@fw1/").unwrap(), "https://admin@fw1/");
}

#[test]
fn a_filled_link_is_checked_too() {
    let (temp, _) = workspace();
    fs::write(
        temp.path().join("Firewalls/fw3.yaml"),
        "address: 10.0.0.3\ncustom:\n  scheme: file\nweb_links:\n  Local: '{{custom.scheme}}:///C:/x'\n  Bare: '{{address}}'\n",
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    let host = resolved(&workspace, "Firewalls/fw3.yaml");
    assert!(matches!(
        host.web_link_url("Local"),
        Err(WebLinkError::NotWeb { scheme, .. }) if scheme == "file"
    ));
    assert!(matches!(
        host.web_link_url("Bare"),
        Err(WebLinkError::NotAUrl { .. })
    ));
}
