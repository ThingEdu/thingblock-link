//! The local gateway's axum router: the HTTP API, the `/ws/arduino` and `/ws/ble` editor
//! sockets and the static `/resources` pack files.

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::routing::{any, get};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::gateway::GatewayContext;
use crate::gateway::http_local::{handler, handler_ble, handler_ws};

/// Builds the router with every HTTP route, CORS, and `context` as shared state.
pub fn router(context: GatewayContext) -> Router {
    // The editor's public origin reaching localhost needs CORS plus Private Network Access.
    // Origin is mirrored because PNA rejects a wildcard origin.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_private_network(true);

    // Packs are redeployed in place under stable URLs, so `no-cache` forces revalidation
    // (`ServeDir` answers 304) instead of serving stale files from heuristic freshness.
    let resources = Router::new()
        .fallback_service(ServeDir::new(context.thingblock.resource_root().path()))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ));

    Router::new()
        .route("/api/platforms", get(handler::list_platforms))
        .route("/api/platforms/{id}", get(handler::platform_status))
        .route("/api/boards", get(handler::list_boards))
        .route("/ws/arduino", any(handler_ws::ws))
        .route("/ws/ble", any(handler_ble::ws))
        .nest("/resources", resources)
        // Routes must be registered before this layer to inherit CORS/PNA.
        .layer(cors)
        .with_state(context)
}
