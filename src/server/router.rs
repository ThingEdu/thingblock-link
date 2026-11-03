//! The axum app: the WS accept loop, the HTTP API, the `/io` BLE channel and the static
//! `/resources` pack files, all on one listener.

use std::future::Future;
use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::extract::ws::WebSocket;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum::routing::{any, get};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{info, warn};

use crate::error::Result;
use crate::server::api;
use crate::server::session::Session;
use crate::service::arduino::daemon::Daemon;
use crate::service::ble::transport::Ble;
use crate::service::resource::ResourceRoot;

/// Shared handles every connection needs: the daemon (gRPC) and the resource root (lib
/// resolution). Cheap to clone since both are `Arc`.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<Daemon>,
    pub(crate) resource_root: Arc<ResourceRoot>,
}

/// Serves WS, HTTP API and resource files on `listener` until shutdown. The caller binds the
/// listener so it can pick the port (or bind `:0` and read it back, as the tests do).
pub async fn serve(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    resource_root: Arc<ResourceRoot>,
) -> Result<()> {
    serve_with_ble(listener, daemon, resource_root, Ble::discover()).await
}

/// [`serve`] with the BLE adapter coming from `discover`. Discovery runs in the background:
/// on macOS it blocks until the user answers the Bluetooth permission prompt, and the
/// resources, compiler and serial faces must not wait on that. `/io` rejects requests until
/// an adapter is ready.
pub async fn serve_with_ble<D>(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    resource_root: Arc<ResourceRoot>,
    discover: D,
) -> Result<()>
where
    D: Future<Output = Result<Option<Ble>>> + Send + 'static,
{
    // The editor's public origin reaching localhost needs CORS plus Private Network Access, which
    // a bare CORS layer doesn't emit. Origin is mirrored because PNA rejects a wildcard origin.
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_private_network(true);

    // A missing radio must not break the rest of the server; `/io` just rejects every request
    // until one is available.
    let ble: Arc<OnceLock<Arc<Ble>>> = Arc::new(OnceLock::new());
    let slot = ble.clone();
    tokio::spawn(async move {
        match discover.await {
            Ok(Some(b)) => {
                info!("ble adapter ready");
                let _ = slot.set(Arc::new(b));
            }
            Ok(None) => info!("no ble adapter; /io will reject requests"),
            Err(e) => warn!(error = %e, "ble init failed; /io disabled"),
        }
    });

    let app = Router::new()
        .route("/", any(ws_handler))
        .route("/api/platforms", get(api::list_platforms))
        .route("/api/platforms/{id}", get(api::platform_status))
        .route(
            "/io",
            any(move |ws: WebSocketUpgrade| crate::service::ble::upgrade(ws, ble.get().cloned())),
        )
        .nest(
            "/resources",
            // Packs are redeployed in place under stable URLs, so heuristic freshness would serve
            // stale files for days. `no-cache` forces revalidation (`ServeDir` answers 304).
            Router::new()
                .fallback_service(ServeDir::new(resource_root.path()))
                .layer(SetResponseHeaderLayer::overriding(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                )),
        )
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

/// Upgrades an HTTP connection to WebSocket and hands the socket to a new [`Session`].
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket: WebSocket| {
        Session::new(state.daemon, state.resource_root).run(socket)
    })
}
