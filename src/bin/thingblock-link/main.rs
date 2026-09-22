//! Entry point for the local helper: builds the tokio runtime, then hands the main thread
//! to the tray UI, which owns startup (daemon + WS server) and the tao event loop.

mod ui;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use clap::Parser;
use thingblock_link::error::Result;
use ui::tray;

/// WS port the editor connects to; a contract with the editor, overridable with `--port`.
const DEFAULT_WS_PORT: u16 = 3030;

#[derive(Parser)]
#[command(
    name = "thingblock-link",
    about = "Local arduino-cli helper for the thingblock-editor"
)]
struct Args {
    /// TCP port for the WebSocket server (bound on localhost).
    #[arg(long, default_value_t = DEFAULT_WS_PORT)]
    port: u16,

    /// Resource pack directory to serve [default: `thingblock-resource` beside the exe].
    #[arg(long)]
    resource_root: Option<PathBuf>,

    /// Path to the `arduino-cli` binary [default: in-tree `arduino-cli-binaries/` copy].
    #[arg(long)]
    arduino_cli: Option<PathBuf>,

    /// Directory with `arduino-cli.yaml` and the daemon's `data/` bundle [default: crate root].
    #[arg(long)]
    config_dir: Option<PathBuf>,
}

/// Packaged default: `thingblock-resource` beside the binary in the install dir.
/// Falls back to a CWD-relative path if the executable location is unavailable.
fn default_resource_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("thingblock-resource")))
        .unwrap_or_else(|| PathBuf::from("thingblock-resource"))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("thingblock_link=info")),
        )
        .init();

    tracing::info!("thingblock-link starting");
    let args = Args::parse();
    let resource_root = args.resource_root.unwrap_or_else(default_resource_root);
    let config_dir = args
        .config_dir
        .unwrap_or_else(thingblock_link::service::arduino::daemon::default_config_dir);

    // tao's event loop must own the main thread, so services run on this runtime and the
    // tray takes over; `tray::run` never returns, the process exits from its loop on Quit.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    tray::run(
        runtime,
        args.port,
        resource_root,
        args.arduino_cli,
        config_dir,
    )
}
