//! Watching a session window's page for its death (#89): a Tauri
//! `Channel::send` into a crashed page still says `Ok`, so the shell asks
//! the webview engine itself. Windows: WebView2's `ProcessFailed`, acted on
//! only when the page's own renderer exited (an unresponsive one may
//! recover; a frame's, the GPU's or a utility process's exit leave the
//! page alive). Linux: WebKitGTK's `web-process-terminated`, any reason.
//! Elsewhere nothing is watched.

/// Calls `died` once the page of `platform` is gone. Runs on the
/// webview's thread (`with_webview`); `died` must not block.
pub(super) fn watch(platform: tauri::webview::PlatformWebview, died: impl Fn() + 'static) {
    platform::watch(platform, died);
}

#[cfg(windows)]
mod platform {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    };
    use webview2_com::ProcessFailedEventHandler;

    pub(crate) fn watch(platform: tauri::webview::PlatformWebview, died: impl Fn() + 'static) {
        // SAFETY: COM calls on the webview's own thread, where
        // `with_webview` runs this.
        unsafe {
            let Ok(core) = platform.controller().CoreWebView2() else {
                return;
            };
            let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                if let Some(args) = args {
                    args.ProcessFailedKind(&mut kind)?;
                }
                // Only the page's own renderer: an unresponsive one may
                // recover, and a frame's, the GPU's or a utility process
                // leave the page alive.
                if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED {
                    died();
                }
                Ok(())
            }));
            let mut token = 0;
            let _ = core.add_ProcessFailed(&handler, &mut token);
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use webkit2gtk::WebViewExt;

    pub(crate) fn watch(platform: tauri::webview::PlatformWebview, died: impl Fn() + 'static) {
        // Crashed, over its memory limit, or ended by the API: gone.
        platform
            .inner()
            .connect_web_process_terminated(move |_, _| died());
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    pub(crate) fn watch(_: tauri::webview::PlatformWebview, _: impl Fn() + 'static) {}
}
