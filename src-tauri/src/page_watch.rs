//! Watching a window's page for its death (#89, #120): a Tauri
//! `Channel::send` into a crashed page still says `Ok`, so the shell asks
//! the webview engine itself. Windows: WebView2's `ProcessFailed`, acted on
//! when the page's own renderer exited, or the engine's browser process
//! (every window's page goes with it). An unresponsive renderer is not
//! acted on: WebView2 did not report one for a page stopped for 48 s
//! (#120), so a hung page is found by the shell's own questions instead
//! (`netterm_core::layout::PageAnswers`); a frame's, the GPU's or a utility
//! process's exit leave the page alive. Linux: WebKitGTK's
//! `web-process-terminated`, any reason, as the page's own. Elsewhere
//! nothing is watched.

/// What became of a watched page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    /// Its own renderer is gone.
    Gone,
    /// The engine as a whole is gone: every window's page with it.
    EngineGone,
}

/// Calls `died` once the page of `platform` is gone. Runs on the
/// webview's thread (`with_webview`); `died` must not block.
pub(super) fn watch(platform: tauri::webview::PlatformWebview, died: impl Fn(Page) + 'static) {
    platform::watch(platform, died);
}

#[cfg(windows)]
mod platform {
    use super::Page;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    };
    use webview2_com::ProcessFailedEventHandler;

    pub(crate) fn watch(platform: tauri::webview::PlatformWebview, died: impl Fn(Page) + 'static) {
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
                if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED {
                    died(Page::Gone);
                } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
                    died(Page::EngineGone);
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
    use super::Page;
    use webkit2gtk::WebViewExt;

    pub(crate) fn watch(platform: tauri::webview::PlatformWebview, died: impl Fn(Page) + 'static) {
        // Crashed, over its memory limit, or ended by the API: gone.
        platform
            .inner()
            .connect_web_process_terminated(move |_, _| died(Page::Gone));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::Page;

    pub(crate) fn watch(_: tauri::webview::PlatformWebview, _: impl Fn(Page) + 'static) {}
}
