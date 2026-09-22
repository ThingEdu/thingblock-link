mod handler;
mod routes;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use thingblock_link::error::Result;
use thingblock_link::service::arduino::daemon::{Daemon, default_config_dir};
use thingblock_link::service::resource::ResourceRoot;
use tokio::net::TcpListener;

const DEFAULT_PORT: u16 = 8080;

#[derive(Parser)]
#[command(
    name = "thingblock-link-cloud",
    about = "Public compile server for the thingblock-editor"
)]
struct Args {
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    #[arg(long)]
    resource_root: Option<PathBuf>,

    #[arg(long)]
    arduino_cli: Option<PathBuf>,

    #[arg(long)]
    config_dir: Option<PathBuf>,
}

fn default_resource_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("thingblock-resource")))
        .unwrap_or_else(|| PathBuf::from("thingblock-resource"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("thingblock_link=info")),
        )
        .init();

    tracing::info!("thingblock-link-cloud starting");
    let args = Args::parse();
    let resource_root = args.resource_root.unwrap_or_else(default_resource_root);
    let config_dir = args.config_dir.unwrap_or_else(default_config_dir);

    let daemon = Arc::new(Daemon::start_with(args.arduino_cli, Some(config_dir)).await?);
    let resource_root = Arc::new(ResourceRoot::new(resource_root)?);
    let listener = TcpListener::bind(("0.0.0.0", args.port)).await?;

    routes::serve(
        listener,
        routes::AppState {
            daemon,
            resource_root,
        },
    )
    .await
}
