//! Composition root: shared state, route table, middleware stack.

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use thingblock_link::error::Result;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::resource::ResourceRoot;
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::info;

use crate::handler::health;

/// Per-caller context (tenant, quota, auth subject) belongs in a request
/// extension set by middleware, not here.
#[derive(Clone)]
pub struct AppState {
    pub daemon: Arc<Daemon>,
    pub resource_root: Arc<ResourceRoot>,
}

/// Split from [`serve`] so tests can drive the router without a socket.
pub fn app(state: AppState) -> Router {
    let resources = ServeDir::new(state.resource_root.path());

    Router::new()
        .route("/health", get(health::health))
        .nest_service("/resources", resources)
        .layer(cors())
        .with_state(state)
}

fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods(Any)
        .allow_headers(Any)
}

pub async fn serve(listener: TcpListener, state: AppState) -> Result<()> {
    info!(addr = ?listener.local_addr()?, "cloud server listening");
    axum::serve(listener, app(state)).await?;
    Ok(())
}
