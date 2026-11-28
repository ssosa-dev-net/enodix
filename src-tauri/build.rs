fn main() {
    // The app's manifest goes in through the linker (below), not through
    // tauri-build's resource file: that one reaches only the app's binary,
    // and the shell's test binary (#70) needs it too.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("tauri-build failed");
    embed_manifest();
}

/// Embeds the app's manifest (Tauri's default: Common Controls v6, which
/// Tauri's dialogs import from) in every binary the crate links: the app,
/// and the shell's tests, which never start without it on Windows
/// (`STATUS_ENTRYPOINT_NOT_FOUND`).
fn embed_manifest() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = out.join("app-manifest.xml");
    std::fs::write(&manifest, APP_MANIFEST).expect("the app's manifest is written");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    // As the resource file had it: no UAC section added by the linker.
    println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
}

/// Tauri's default application manifest (`tauri-build`'s
/// `windows-app-manifest.xml`), as the app had it.
const APP_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
