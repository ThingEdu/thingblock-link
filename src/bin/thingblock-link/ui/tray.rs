//! System-tray status/quit UI and the helper's main-thread event loop. The async side
//! (daemon + WS server) talks to the loop only through its `EventLoopProxy`.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tracing::{error, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::ui::window::{self, StatusView, StatusWindow, Telemetry};
use thingblock_link::server;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::resource::ResourceRoot;

/// How often the status window's board-count / health rows are refreshed.
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(3);

/// What the tray reports about the helper's lifecycle.
enum Status {
    Starting,
    Running(u16),
    Failed(String),
}

impl Status {
    /// Text for the disabled status line in the tray menu.
    fn label(&self) -> String {
        match self {
            Status::Starting => "Starting…".into(),
            Status::Running(port) => format!("Running on :{port}"),
            Status::Failed(msg) => format!("Failed: {msg}"),
        }
    }

    /// Hover tooltip on the tray icon itself.
    fn tooltip(&self) -> String {
        format!("thingblock-link — {}", self.label())
    }

    /// Serializable snapshot of this status for the status window's status row.
    fn view(&self) -> StatusView {
        match self {
            Status::Starting => StatusView {
                state: "starting",
                port: None,
                message: None,
            },
            Status::Running(port) => StatusView {
                state: "running",
                port: Some(*port),
                message: None,
            },
            Status::Failed(msg) => StatusView {
                state: "failed",
                port: None,
                message: Some(msg.clone()),
            },
        }
    }
}

/// Events that wake the loop. The async side and menu clicks both funnel through here
/// so the loop can idle on `ControlFlow::Wait` instead of busy-polling.
enum UserEvent {
    Status(Status),
    Telemetry(Telemetry),
    MenuClick(MenuId),
    /// The status window's Quit button, bridged from its IPC channel.
    Quit,
}

/// Tray handles kept alive for the loop's lifetime: the status item so its text can be
/// updated, the menu ids to match clicks against.
struct Tray {
    _icon: TrayIcon,
    status_item: MenuItem,
    show_id: MenuId,
    quit_id: MenuId,
}

/// Runs the helper: spawns the services on `runtime`, then drives the tray event loop.
/// Must be called on the main thread (tao/tray-icon requirement); exits the process on Quit.
pub fn run(
    runtime: Runtime,
    port: u16,
    resource_root: PathBuf,
    cli_path: Option<PathBuf>,
    config_dir: PathBuf,
) -> ! {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Bridge muda's global menu-event channel into the loop as a user event.
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_proxy.send_event(UserEvent::MenuClick(event.id));
    }));

    // Kept so the status window's Quit button can be wired up when it is lazily built.
    let ipc_proxy = proxy.clone();

    let stdin_proxy = proxy.clone();
    runtime.spawn(watch_stdin_for_shutdown(stdin_proxy));

    // Daemon + WS server + telemetry poller; status flows back through the proxy.
    runtime.spawn(run_services(
        port,
        resource_root,
        cli_path,
        config_dir,
        proxy,
    ));

    // Tray is built on `Init`: macOS requires icon creation after the loop starts.
    // `runtime` sits in an Option so Quit can take it exactly once.
    let mut tray: Option<Tray> = None;
    let mut runtime = Some(runtime);

    // Status window is built lazily and hidden on close for instant reopen; the latest
    // status/telemetry are cached so a freshly built window paints current state at once.
    let mut window: Option<StatusWindow> = None;
    let mut last_status = Status::Starting.view();
    let mut last_telemetry = Telemetry::default();

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => tray = Some(build_tray()),
            Event::UserEvent(UserEvent::Status(status)) => {
                last_status = status.view();
                if let Some(tray) = &tray {
                    tray.status_item.set_text(status.label());
                    let _ = tray._icon.set_tooltip(Some(status.tooltip()));
                    let _ = tray._icon.set_icon(Some(icon_for(&status)));
                }
                if let Some(window) = &window {
                    window.update_status(&last_status);
                }
            }
            Event::UserEvent(UserEvent::Telemetry(telemetry)) => {
                last_telemetry = telemetry;
                if let Some(window) = &window {
                    window.update_telemetry(telemetry);
                }
            }
            Event::UserEvent(UserEvent::Quit) => shutdown(&mut runtime, control_flow),
            Event::UserEvent(UserEvent::MenuClick(id)) => {
                let tray_ref = tray.as_ref();
                if tray_ref.is_some_and(|t| t.quit_id == id) {
                    shutdown(&mut runtime, control_flow);
                } else if tray_ref.is_some_and(|t| t.show_id == id) {
                    if window.is_none() {
                        let quit_proxy = ipc_proxy.clone();
                        window = Some(window::build(
                            target,
                            move |msg| {
                                if msg == "quit" {
                                    let _ = quit_proxy.send_event(UserEvent::Quit);
                                }
                            },
                            &last_status,
                            last_telemetry,
                        ));
                    }
                    if let Some(window) = &window {
                        window.show();
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
                ..
            } if window.as_ref().is_some_and(|w| w.id() == window_id) => {
                // Closing only hides the window; Quit is the sole path that stops the helper.
                if let Some(window) = &window {
                    window.hide();
                }
            }
            _ => {}
        }
    })
}

/// Quits on the desktop shell's shutdown signal: any stdin bytes or EOF. Stdin is portable
/// (no OS signal is) and, unlike the WS server, can't get stuck behind request handling.
async fn watch_stdin_for_shutdown(proxy: EventLoopProxy<UserEvent>) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 64];
    let _ = tokio::io::stdin().read(&mut buf).await;
    info!("stdin shutdown signal received");
    let _ = proxy.send_event(UserEvent::Quit);
}

/// Tears down the helper and exits the loop. Dropping the runtime drops the last
/// `Arc<Daemon>`, whose `kill_on_drop` reaps the arduino-cli daemon.
fn shutdown(runtime: &mut Option<Runtime>, control_flow: &mut ControlFlow) {
    info!("quit requested; shutting down");
    if let Some(runtime) = runtime.take() {
        runtime.shutdown_background();
    }
    *control_flow = ControlFlow::Exit;
}

/// Starts the daemon and serves the editor over WS, reporting lifecycle to the tray.
/// On failure it reports `Failed` and returns; the process stays up so Quit still works.
async fn run_services(
    port: u16,
    resource_root: PathBuf,
    cli_path: Option<PathBuf>,
    config_dir: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
) {
    let _ = proxy.send_event(UserEvent::Status(Status::Starting));

    // Fail fast on a missing pack dir: it's a packaging error, and the editor can't load
    // resources or compile vendored libs without it.
    let resource_root = match ResourceRoot::new(&resource_root) {
        Ok(root) => Arc::new(root),
        Err(e) => {
            error!(error = %e, path = %resource_root.display(), "resource root invalid");
            let _ = proxy.send_event(UserEvent::Status(Status::Failed(e.to_string())));
            return;
        }
    };

    let daemon = match Daemon::start_with(cli_path, Some(config_dir)).await {
        Ok(daemon) => Arc::new(daemon),
        Err(e) => {
            error!(error = %e, "daemon failed to start");
            let _ = proxy.send_event(UserEvent::Status(Status::Failed(e.to_string())));
            return;
        }
    };

    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(listener) => listener,
        Err(e) => {
            error!(error = %e, port, "failed to bind WS port");
            let _ = proxy.send_event(UserEvent::Status(Status::Failed(e.to_string())));
            return;
        }
    };

    let _ = proxy.send_event(UserEvent::Status(Status::Running(port)));

    tokio::spawn(poll_telemetry(daemon.clone(), proxy.clone()));

    if let Err(e) = server::router::serve(listener, daemon, resource_root).await {
        error!(error = %e, "ws server stopped");
        let _ = proxy.send_event(UserEvent::Status(Status::Failed(e.to_string())));
    }
}

/// Periodically reports connected-board count and daemon health to the status window.
/// A failed probe means the daemon is unresponsive; a send error means the loop is gone.
async fn poll_telemetry(daemon: Arc<Daemon>, proxy: EventLoopProxy<UserEvent>) {
    let mut interval = tokio::time::interval(TELEMETRY_INTERVAL);
    loop {
        interval.tick().await;
        let telemetry = match daemon.client().connected_board_count().await {
            Ok(boards) => Telemetry {
                boards,
                healthy: true,
            },
            Err(e) => {
                warn!(error = %e, "board count poll failed");
                Telemetry {
                    boards: 0,
                    healthy: false,
                }
            }
        };
        if proxy.send_event(UserEvent::Telemetry(telemetry)).is_err() {
            break;
        }
    }
}

/// Builds the tray icon and menu (status line, separator, Show Status, Quit). Panics on
/// failure: the OS rejecting the tray is unrecoverable for this UI.
fn build_tray() -> Tray {
    let status_item = MenuItem::new(Status::Starting.label(), false, None);
    let show_item = MenuItem::new("Show Status", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &show_item,
        &quit_item,
    ])
    .expect("build tray menu");

    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(Status::Starting.tooltip())
        .with_icon(icon_for(&Status::Starting))
        .build()
        .expect("build tray icon");

    Tray {
        _icon: icon,
        status_item,
        show_id: show_item.id().clone(),
        quit_id: quit_item.id().clone(),
    }
}

/// The ThingBlock chip glyph as RGBA, the base for the per-status [`icon_for`] variants.
/// Embedded at compile time so the binary stays self-contained.
fn glyph_rgba() -> image::RgbaImage {
    const PNG: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/brand/icons/icon-32.png"
    ));
    image::load_from_memory(PNG)
        .expect("decode embedded tray icon png")
        .into_rgba8()
}

/// Tray icon for a status, a glanceable supplement to the menu line (see `brand/DESIGN.md`):
/// full color when running, 60% opacity when starting, error dot when failed.
fn icon_for(status: &Status) -> Icon {
    let mut image = glyph_rgba();
    match status {
        Status::Running(_) => {}
        Status::Starting => {
            for pixel in image.pixels_mut() {
                pixel[3] = (f32::from(pixel[3]) * 0.6) as u8;
            }
        }
        Status::Failed(_) => overlay_error_dot(&mut image),
    }
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).expect("build tray icon image")
}

/// Paints the editor's error red as a dot over the pin-1 (top-left) corner.
fn overlay_error_dot(image: &mut image::RgbaImage) {
    const RED: image::Rgba<u8> = image::Rgba([0xFF, 0x66, 0x1A, 0xFF]);
    const CENTER: i32 = 6;
    const RADIUS: i32 = 5;
    for y in 0..image.height() as i32 {
        for x in 0..image.width() as i32 {
            let (dx, dy) = (x - CENTER, y - CENTER);
            if dx * dx + dy * dy <= RADIUS * RADIUS {
                image.put_pixel(x as u32, y as u32, RED);
            }
        }
    }
}
