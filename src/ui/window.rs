//! The status window: a `wry` WebView, opened from the tray, showing live helper
//! state (status + WS port, arduino-cli health, connected-board count).
//!
//! The window is owned by the tao event loop in [`crate::ui::tray`] — this module
//! only knows how to *build* one and push state into it. It stays ignorant of
//! the loop's `UserEvent`: the Quit button is wired through a host-supplied IPC
//! callback ([`build`]'s `on_ipc`), so there is no dependency back on `tray`.

use serde::Serialize;
use tao::dpi::LogicalSize;
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder, WindowId};
use tracing::warn;
use wry::http::Request;
use wry::{WebView, WebViewBuilder};

/// Serializable snapshot of the helper's lifecycle for the status row. Mirrors
/// the tray's `Status`; the `state` discriminant matches the strings the page's
/// `__status` handler switches on.
#[derive(Clone, Serialize)]
pub struct StatusView {
    /// `"starting"`, `"running"`, or `"failed"`.
    pub state: &'static str,
    pub port: Option<u16>,
    pub message: Option<String>,
}

/// Health + board-count snapshot for the lower rows, refreshed by the tray's
/// telemetry poller. The [`Default`] (`0` boards, not healthy) is the
/// before-first-poll state: nothing confirmed yet, daemon not yet probed.
#[derive(Clone, Copy, Default, Serialize)]
pub struct Telemetry {
    pub boards: usize,
    pub healthy: bool,
}

/// The open status window: its tao [`Window`] plus the [`WebView`] painted over
/// it. The webview is declared first so it drops before the window it renders
/// into (GTK requires the window to outlive the webview).
pub struct StatusWindow {
    webview: WebView,
    window: Window,
}

impl StatusWindow {
    /// The window's id, for matching `CloseRequested` in the event loop.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Bring the (possibly hidden) window back to the foreground.
    pub fn show(&self) {
        self.window.set_visible(true);
        self.window.set_focus();
    }

    /// Hide without destroying, so the next open is instant and the webview keeps
    /// its state. Used on `CloseRequested` — closing the window must not stop the
    /// helper.
    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Repaint the status row.
    pub fn update_status(&self, status: &StatusView) {
        self.eval("__status", status);
    }

    /// Repaint the health + board-count rows.
    pub fn update_telemetry(&self, telemetry: Telemetry) {
        self.eval("__telemetry", &telemetry);
    }

    /// Call a page-global `fn_name(<json>)` with a serialized payload.
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

/// Build and show the status window on `target`, painting `status`/`telemetry`
/// as its initial state. `on_ipc` receives every `window.ipc.postMessage(..)`
/// body from the page (currently just `"quit"`); the host turns that into a
/// loop event. Generic over the loop's user-event type so this module needn't
/// know it.
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

/// Realize the webview against the tao window. On Linux/BSD (GTK) wry can't take
/// the raw window handle (`UnsupportedWindowHandle`) and must build into the
/// window's GTK container; the other platforms build from the handle directly.
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

/// Assemble the page from the embedded template: inline the brand glyph and the
/// initial state so the window paints correctly the instant it loads (before any
/// live `evaluate_script` update arrives).
fn render_html(status: &StatusView, telemetry: Telemetry) -> String {
    const TEMPLATE: &str = include_str!("../../assets/status.html");
    const GLYPH: &str = include_str!("../../brand/thingblock-icon.svg");

    let status_json = serde_json::to_string(status).expect("serialize initial status");
    let telemetry_json = serde_json::to_string(&telemetry).expect("serialize initial telemetry");

    TEMPLATE
        .replace("__GLYPH_SVG__", GLYPH)
        .replace("__INIT_STATUS__", &status_json)
        .replace("__INIT_TELEMETRY__", &telemetry_json)
}
