//! Composition root: shared state, route table, middleware stack.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use thingblock_link::error::Result;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::resource::ResourceRoot;
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use tracing::info;

use crate::handler::{compile, health};

/// Caps the untrusted sketch payload; generated sources are kilobytes.
const MAX_COMPILE_BODY: usize = 1024 * 1024;

/// Process-wide state shared by all handlers. Per-caller context (tenant, quota, auth)
/// belongs in a middleware-set request extension, not here.
#[derive(Clone)]
pub struct AppState {
    pub daemon: Arc<Daemon>,
    pub resource_root: Arc<ResourceRoot>,
}

/// Builds the router: `/health`, `/compile`, and static `/resources`, behind CORS.
/// Split from [`serve`] so tests can drive the router without a socket.
pub fn app(state: AppState) -> Router {
    let resources = ServeDir::new(state.resource_root.path());

    Router::new()
        .route("/health", get(health::health))
        .route(
            "/compile",
            post(compile::compile).route_layer(RequestBodyLimitLayer::new(MAX_COMPILE_BODY)),
        )
        .nest_service("/resources", resources)
        .layer(cors())
        .with_state(state)
}

/// Permissive CORS (mirrors the request origin) so any editor origin can call the server.
fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Serves the app on `listener` until the server stops.
pub async fn serve(listener: TcpListener, state: AppState) -> Result<()> {
    info!(addr = ?listener.local_addr()?, "cloud server listening");
    axum::serve(listener, app(state)).await?;
    Ok(())
}
