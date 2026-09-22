//! Status window: a `wry` WebView showing live helper state. Ignorant of the tray's
//! event type; Quit is wired through a host-supplied IPC callback, so no dependency on `tray`.

use serde::Serialize;
use tao::dpi::LogicalSize;
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder, WindowId};
use tracing::warn;
use wry::http::Request;
use wry::{WebView, WebViewBuilder};

/// Lifecycle snapshot for the status row, mirroring the tray's `Status`.
/// JSON shape is a contract with `assets/status.html`'s `__status` handler.
#[derive(Clone, Serialize)]
pub struct StatusView {
    /// `"starting"`, `"running"`, or `"failed"`.
    pub state: &'static str,
    pub port: Option<u16>,
    pub message: Option<String>,
}

/// Health + board-count snapshot from the tray's poller. `Default` (0 boards, unhealthy)
/// is the before-first-poll state.
#[derive(Clone, Copy, Default, Serialize)]
pub struct Telemetry {
    pub boards: usize,
    pub healthy: bool,
}

/// The open status window: a tao window plus the webview painted over it.
pub struct StatusWindow {
    // Field order matters: webview drops first, as GTK requires the window to outlive it.
    webview: WebView,
    window: Window,
}

impl StatusWindow {
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Brings the (possibly hidden) window back to the foreground.
    pub fn show(&self) {
        self.window.set_visible(true);
        self.window.set_focus();
    }

    /// Hides rather than destroys on close, so reopen is instant and the webview keeps its
    /// state; closing the window must not stop the helper.
    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Repaints the status row.
    pub fn update_status(&self, status: &StatusView) {
        self.eval("__status", status);
    }

    /// Repaints the health + board-count rows.
    pub fn update_telemetry(&self, telemetry: Telemetry) {
        self.eval("__telemetry", &telemetry);
    }

    /// Calls the page-global `window.<fn_name>(<json>)`; failures are logged, not fatal.
    fn eval<T: Serialize>(&self, fn_name: &str, payload: &T) {
        let json = serde_json::to_string(payload).expect("serialize window payload");
        if let Err(e) = self
            .webview
            .evaluate_script(&format!("window.{fn_name}({json})"))
        {
            warn!(error = %e, fn_name, "status window script eval failed");
        }
    }
}

/// Builds the status window painted with the given initial state. `on_ipc` receives each
/// `window.ipc.postMessage` body (just `"quit"`); generic over `T` so it needn't know the loop.
pub fn build<T: 'static>(
    target: &EventLoopWindowTarget<T>,
    on_ipc: impl Fn(String) + 'static,
    status: &StatusView,
    telemetry: Telemetry,
) -> StatusWindow {
    let window = WindowBuilder::new()
        .with_title("ThingBlock Link")
        .with_inner_size(LogicalSize::new(340.0, 240.0))
        .with_resizable(false)
        .build(target)
        .expect("build status window");

    let builder = WebViewBuilder::new()
        .with_html(render_html(status, telemetry))
        .with_ipc_handler(move |req: Request<String>| on_ipc(req.into_body()));

    let webview = build_webview(builder, &window);

    StatusWindow { webview, window }
}

/// Realizes the webview in the tao window. On GTK wry can't take the raw window handle
/// (`UnsupportedWindowHandle`), so it builds into the window's vbox instead.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
))]
fn build_webview(builder: WebViewBuilder<'_>, window: &Window) -> WebView {
    use tao::platform::unix::WindowExtUnix;
    use wry::WebViewBuilderExtUnix;

    let vbox = window
        .default_vbox()
        .expect("tao window exposes a GTK vbox on Linux/BSD");
    builder.build_gtk(vbox).expect("build status webview")
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
)))]
fn build_webview(builder: WebViewBuilder<'_>, window: &Window) -> WebView {
    builder.build(window).expect("build status webview")
}

/// Fills the embedded page template with the brand glyph and initial state, so the page
/// paints correctly before any `evaluate_script` update arrives.
fn render_html(status: &StatusView, telemetry: Telemetry) -> String {
    const TEMPLATE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/status.html"));
    const GLYPH: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/brand/thingblock-icon.svg"
    ));

    let status_json = serde_json::to_string(status).expect("serialize initial status");
    let telemetry_json = serde_json::to_string(&telemetry).expect("serialize initial telemetry");

    TEMPLATE
        .replace("__GLYPH_SVG__", GLYPH)
        .replace("__INIT_STATUS__", &status_json)
        .replace("__INIT_TELEMETRY__", &telemetry_json)
}
