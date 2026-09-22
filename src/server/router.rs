use std::sync::Arc;

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
use crate::service::resource::ResourceRoot;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<Daemon>,
    pub(crate) resource_root: Arc<ResourceRoot>,
}

pub async fn serve(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    resource_root: Arc<ResourceRoot>,
) -> Result<()> {
    // Public editor origin → localhost needs Private Network Access; PNA rejects a wildcard origin.
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_private_network(true);

    // No BLE adapter must not break the server; `/io` just rejects requests.
    let ble = match crate::service::ble::transport::Ble::discover().await {
        Ok(Some(b)) => {
            info!("ble adapter ready");
            Some(Arc::new(b))
        }
        Ok(None) => {
            info!("no ble adapter; /io will reject requests");
            None
        }
        Err(e) => {
            warn!(error = %e, "ble init failed; /io disabled");
            None
        }
    };

    let app = Router::new()
        .route("/", any(ws_handler))
        .route("/api/platforms", get(api::list_platforms))
        .route("/api/platforms/{id}", get(api::platform_status))
        .route(
            "/io",
            any(move |ws: WebSocketUpgrade| crate::service::ble::upgrade(ws, ble.clone())),
        )
        .nest(
            "/resources",
            // Packs are redeployed in place under stable URLs; heuristic freshness would serve stale files.
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

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket: WebSocket| {
        Session::new(state.daemon, state.resource_root).run(socket)
    })
}
