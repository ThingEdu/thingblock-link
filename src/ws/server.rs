//! WS accept loop and the static resource route. Serves `ws://localhost:PORT`
//! (localhost is a secure context, so no TLS pre-M5) on a caller-provided
//! listener, upgrades each connection, and drives a [`Session`] per socket. The
//! same listener also serves the resource root over HTTP at `/resources` so the
//! editor can `import()` pack files — a sandboxed browser can't read the helper's
//! filesystem, so HTTP is the only handle it has (see [`crate::resource`]).

use std::sync::Arc;

use axum::Router;
use axum::extract::ws::WebSocket;
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::{any, get};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::info;

use crate::daemon::Daemon;
use crate::error::Result;
use crate::resource::ResourceRoot;
use crate::ws::api;
use crate::ws::session::Session;

/// Shared, immutable handles every connection needs: the daemon (gRPC) and the
/// resource root (compile lib resolution). Cheap to clone — both are `Arc`.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<Daemon>,
    pub(crate) resource_root: Arc<ResourceRoot>,
}

/// Serve WS connections and the resource files on `listener` until shutdown,
/// sharing one daemon handle and one resource root across sessions. The caller
/// binds the listener so it can choose the port (or bind `:0` and read the
/// assigned address, as the tests do).
pub async fn serve(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    resource_root: Arc<ResourceRoot>,
) -> Result<()> {
    // The editor is served from a different (public) origin than this localhost
    // helper, so the static route needs CORS — and, because the request crosses
    // into the local network, Private Network Access: Chromium's public→local
    // preflight expects `Access-Control-Allow-Private-Network: true`, which a
    // bare CORS layer does not emit. Origin is mirrored rather than `Any` because
    // PNA is incompatible with a wildcard origin.
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_private_network(true);

    let app = Router::new()
        .route("/", any(ws_handler))
        .route("/api/platforms", get(api::list_platforms))
        .route("/api/platforms/{id}", get(api::platform_status))
        .nest_service("/resources", ServeDir::new(resource_root.path()))
        // Routes must be registered before this layer to inherit CORS/PNA.
        .layer(cors)
        .with_state(AppState {
            daemon,
            resource_root,
        });

    info!(addr = ?listener.local_addr()?, "ws server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Upgrade an HTTP connection to WebSocket and hand the socket to a `Session`.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket: WebSocket| {
        Session::new(state.daemon, state.resource_root).run(socket)
    })
}
