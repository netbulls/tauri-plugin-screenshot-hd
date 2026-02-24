//! # tauri-plugin-screenshot-hd
//!
//! Pixel-perfect screenshot server for Tauri apps.
//!
//! Starts a lightweight HTTP server (debug builds only, unless the `release`
//! feature is enabled) that exposes:
//!
//! - `GET /screenshot` — native WKWebView.takeSnapshot on macOS (PNG bytes)
//! - `POST /eval` — execute JavaScript in the webview
//! - `POST /eval?wait=N` — execute JS, wait N ms, then return a screenshot
//!
//! ## Usage
//!
//! ```rust,no_run
//! // In your lib.rs or main.rs:
//! let mut builder = tauri::Builder::default();
//!
//! #[cfg(debug_assertions)]
//! {
//!     builder = builder.plugin(tauri_plugin_screenshot_hd::init());
//! }
//! ```
//!
//! Then take screenshots with:
//! ```bash
//! curl -s http://127.0.0.1:21988/screenshot -o screenshot.png
//! ```

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

use std::io::Read;
use std::sync::OnceLock;
use tauri::{
    plugin::{Builder as PluginBuilder, TauriPlugin},
    Manager, Runtime,
};

const DEFAULT_PORT: u16 = 21988;
const DEFAULT_HOST: &str = "127.0.0.1";

/// Configuration for the screenshot server.
#[derive(Debug, Clone)]
pub struct Config {
    /// Host to bind to. Default: `127.0.0.1`
    pub host: String,
    /// Port to listen on. Default: `21988`
    pub port: u16,
    /// Name of the webview window to capture. Default: `main`
    pub window_label: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            window_label: "main".to_string(),
        }
    }
}

/// Initialize the plugin with default config.
///
/// Binds to `127.0.0.1:21988` and captures the `main` window.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    init_with(Config::default())
}

/// Initialize the plugin with custom config.
pub fn init_with<R: Runtime>(config: Config) -> TauriPlugin<R> {
    PluginBuilder::new("screenshot-hd")
        .setup(move |app, _api| {
            // Respect debug-only default: skip in release unless feature is set
            #[cfg(not(feature = "release"))]
            if !cfg!(debug_assertions) {
                return Ok(());
            }

            let app_handle = app.clone();
            let addr = format!("{}:{}", config.host, config.port);

            std::thread::spawn(move || {
                let server = match tiny_http::Server::http(&addr) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("[screenshot-hd] failed to start on {addr}: {e}");
                        return;
                    }
                };
                log::info!("[screenshot-hd] listening on http://{addr}");

                serve_loop(server, app_handle, config.window_label);
            });

            Ok(())
        })
        .build()
}

/// Main HTTP server loop.
///
/// The window is resolved lazily on first request — this avoids the race
/// condition where the plugin's `setup` runs before windows are created.
fn serve_loop<R: Runtime>(
    server: tiny_http::Server,
    app_handle: tauri::AppHandle<R>,
    window_label: String,
) {
    let window_cell: OnceLock<tauri::WebviewWindow<R>> = OnceLock::new();

    loop {
        let mut request = match server.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(Some(r)) => r,
            Ok(None) | Err(_) => continue,
        };

        // Lazy window lookup
        let window = match window_cell.get() {
            Some(w) => w,
            None => {
                match app_handle.get_webview_window(&window_label) {
                    Some(w) => {
                        let _ = window_cell.set(w);
                        window_cell.get().unwrap()
                    }
                    None => {
                        let resp = tiny_http::Response::from_string(format!(
                            "window '{}' not found yet — app may still be starting",
                            window_label
                        ))
                        .with_status_code(503);
                        let _ = request.respond(resp);
                        continue;
                    }
                }
            }
        };

        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or(&url);

        match path {
            "/screenshot" => {
                match take_screenshot(window) {
                    Ok(bytes) => {
                        let resp = tiny_http::Response::from_data(bytes).with_header(
                            "Content-Type: image/png"
                                .parse::<tiny_http::Header>()
                                .unwrap(),
                        );
                        let _ = request.respond(resp);
                    }
                    Err(e) => {
                        log::error!("[screenshot-hd] capture failed: {e}");
                        let resp = tiny_http::Response::from_string(e).with_status_code(504);
                        let _ = request.respond(resp);
                    }
                }
            }

            "/eval" => {
                let mut body = String::new();
                if let Err(e) = request.as_reader().read_to_string(&mut body) {
                    let resp = tiny_http::Response::from_string(format!("read error: {e}"))
                        .with_status_code(400);
                    let _ = request.respond(resp);
                    continue;
                }

                if let Err(e) = window.eval(&body) {
                    let resp = tiny_http::Response::from_string(format!("eval error: {e}"))
                        .with_status_code(500);
                    let _ = request.respond(resp);
                    continue;
                }

                // ?wait=N — wait N ms then return screenshot
                let wait_ms: Option<u64> = url
                    .split('?')
                    .nth(1)
                    .and_then(|qs| {
                        qs.split('&')
                            .find(|p| p.starts_with("wait="))
                            .and_then(|p| p[5..].parse().ok())
                    });

                if let Some(ms) = wait_ms {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    match take_screenshot(window) {
                        Ok(bytes) => {
                            let resp = tiny_http::Response::from_data(bytes).with_header(
                                "Content-Type: image/png"
                                    .parse::<tiny_http::Header>()
                                    .unwrap(),
                            );
                            let _ = request.respond(resp);
                        }
                        Err(e) => {
                            let resp =
                                tiny_http::Response::from_string(e).with_status_code(504);
                            let _ = request.respond(resp);
                        }
                    }
                } else {
                    let resp = tiny_http::Response::from_string("ok");
                    let _ = request.respond(resp);
                }
            }

            _ => {
                let resp = tiny_http::Response::from_string(
                    "tauri-plugin-screenshot-hd\n\n\
                     GET  /screenshot        — capture PNG\n\
                     POST /eval              — run JS in webview\n\
                     POST /eval?wait=<ms>    — run JS, wait, then capture PNG",
                )
                .with_status_code(404);
                let _ = request.respond(resp);
            }
        }
    }
}

// ── macOS: native WKWebView.takeSnapshot ─────────────────────────────

#[cfg(target_os = "macos")]
fn take_screenshot<R: Runtime>(window: &tauri::WebviewWindow<R>) -> Result<Vec<u8>, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>, String>>();

    window
        .with_webview(move |platform_webview| {
            unsafe {
                let wk_webview: cocoa::base::id = platform_webview.inner() as cocoa::base::id;

                let block = block::ConcreteBlock::new(
                    move |ns_image: cocoa::base::id, ns_error: cocoa::base::id| {
                        if ns_image == cocoa::base::nil {
                            let desc: cocoa::base::id =
                                objc::msg_send![ns_error, localizedDescription];
                            let cstr: *const std::os::raw::c_char =
                                objc::msg_send![desc, UTF8String];
                            let msg = if cstr.is_null() {
                                "takeSnapshot failed".to_string()
                            } else {
                                std::ffi::CStr::from_ptr(cstr)
                                    .to_string_lossy()
                                    .into_owned()
                            };
                            let _ = tx.send(Err(msg));
                            return;
                        }

                        // NSImage → TIFF → NSBitmapImageRep → PNG
                        let tiff_data: cocoa::base::id =
                            objc::msg_send![ns_image, TIFFRepresentation];
                        if tiff_data == cocoa::base::nil {
                            let _ = tx.send(Err("TIFFRepresentation nil".into()));
                            return;
                        }

                        let alloc: cocoa::base::id =
                            objc::msg_send![objc::class!(NSBitmapImageRep), alloc];
                        let bitmap_rep: cocoa::base::id =
                            objc::msg_send![alloc, initWithData: tiff_data];
                        if bitmap_rep == cocoa::base::nil {
                            let _ = tx.send(Err("NSBitmapImageRep nil".into()));
                            return;
                        }

                        let png_type: u64 = 4; // NSBitmapImageFileTypePNG
                        let empty_dict: cocoa::base::id =
                            objc::msg_send![objc::class!(NSDictionary), dictionary];
                        let png_data: cocoa::base::id = objc::msg_send![
                            bitmap_rep,
                            representationUsingType: png_type
                            properties: empty_dict
                        ];

                        if png_data == cocoa::base::nil {
                            let _: () = objc::msg_send![bitmap_rep, release];
                            let _ = tx.send(Err("PNG conversion nil".into()));
                            return;
                        }

                        let length: usize = objc::msg_send![png_data, length];
                        let bytes_ptr: *const u8 = objc::msg_send![png_data, bytes];
                        let png_bytes =
                            std::slice::from_raw_parts(bytes_ptr, length).to_vec();
                        let _: () = objc::msg_send![bitmap_rep, release];
                        let _ = tx.send(Ok(png_bytes));
                    },
                );
                let block = block.copy();

                let _: () = objc::msg_send![
                    wk_webview,
                    takeSnapshotWithConfiguration: cocoa::base::nil
                    completionHandler: &*block
                ];
            }
        })
        .map_err(|e| format!("with_webview: {e}"))?;

    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| format!("snapshot timeout: {e}"))?
}

// ── Non-macOS: stub that returns an error ────────────────────────────

#[cfg(not(target_os = "macos"))]
fn take_screenshot<R: Runtime>(_window: &tauri::WebviewWindow<R>) -> Result<Vec<u8>, String> {
    Err("Native screenshots are only supported on macOS (WKWebView.takeSnapshot). \
         On other platforms, use the WebDriver screenshot endpoint instead."
        .into())
}
