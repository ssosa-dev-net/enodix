//! The app's identifier and name come from the Tauri configuration, where
//! they are defined once: the binary needs the identifier to find the
//! app's data folder and keychain entries, and the name for its messages.
//! On Windows the program also gets the app's version details: its name,
//! publisher and copyright line (as `scripts/release.mjs` makes it for the
//! app and the installer), a description and the version, and the VC++
//! runtime linked in.

use std::path::Path;

fn main() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/tauri.conf.json");
    println!("cargo:rerun-if-changed={}", config.display());
    let text = std::fs::read_to_string(&config).expect("src-tauri/tauri.conf.json is readable");
    let config: serde_json::Value = serde_json::from_str(&text).expect("tauri.conf.json is JSON");
    let text_of = |value: &serde_json::Value, key: &str| -> String {
        value
            .as_str()
            .unwrap_or_else(|| panic!("tauri.conf.json has no {key}"))
            .to_owned()
    };
    let product = text_of(&config["productName"], "productName");
    for (key, variable) in [
        ("identifier", "NETTERM_IDENTIFIER"),
        ("productName", "NETTERM_PRODUCT_NAME"),
    ] {
        println!("cargo:rustc-env={variable}={}", text_of(&config[key], key));
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let publisher = text_of(&config["bundle"]["publisher"], "bundle.publisher");
        version_details(&product, &publisher);
        static_runtime();
    }
}

/// The VC++ runtime linked into the program (the Universal CRT stays a
/// part of Windows), as Tauri does for the app: a clean Windows has no
/// `vcruntime140.dll`, and the program would not start there (seen in
/// Windows Sandbox, #139).
#[cfg(windows)]
fn static_runtime() {
    static_vcruntime::metabuild();
}

#[cfg(not(windows))]
fn static_runtime() {}

/// The version details Windows shows in the program's properties.
#[cfg(windows)]
fn version_details(product: &str, publisher: &str) {
    let name = env!("CARGO_PKG_NAME");
    let mut resource = tauri_winres::WindowsResource::new();
    resource
        .set("ProductName", product)
        .set(
            "FileDescription",
            &format!("{product} from the command line"),
        )
        .set("CompanyName", publisher)
        .set("LegalCopyright", &format!("Copyright (C) 2026 {publisher}"))
        .set("InternalName", name)
        .set("OriginalFilename", &format!("{name}.exe"));
    resource
        .compile()
        .expect("the version details of netterm-cli.exe compile");
}

/// Built elsewhere for Windows (cross-compiling): no version details.
#[cfg(not(windows))]
fn version_details(_product: &str, _publisher: &str) {}
