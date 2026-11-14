//! The app's identifier and name come from the Tauri configuration, where
//! they are defined once: the binary needs the identifier to find the
//! app's data folder and keychain entries, and the name for its messages.

use std::path::Path;

fn main() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/tauri.conf.json");
    println!("cargo:rerun-if-changed={}", config.display());
    let text = std::fs::read_to_string(&config).expect("src-tauri/tauri.conf.json is readable");
    let config: serde_json::Value = serde_json::from_str(&text).expect("tauri.conf.json is JSON");
    for (key, variable) in [
        ("identifier", "NETTERM_IDENTIFIER"),
        ("productName", "NETTERM_PRODUCT_NAME"),
    ] {
        let value = config[key]
            .as_str()
            .unwrap_or_else(|| panic!("tauri.conf.json has no {key}"));
        println!("cargo:rustc-env={variable}={value}");
    }
}
