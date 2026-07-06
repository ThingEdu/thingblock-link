//! HTTP JSON API for one-shot reads — currently the platform install status
//! behind the editor's board-manager flow. A one-shot request/response fits
//! plain HTTP better than the WS envelope (curl-able, no socket or id
//! correlation needed); the streaming install itself stays on the WS
//! (`installPlatform`). Routes share the WS listener and inherit its CORS/PNA
//! layer (see [`crate::server::router`]).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;

use crate::error::Error;
use crate::server::router::AppState;

/// Error responses reuse the WS terminal's `{code, message}` shape.
type ApiError = (StatusCode, Json<serde_json::Value>);

/// `GET /api/platforms` — every indexed platform with its install status.
pub(crate) async fn list_platforms(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let platforms = state
        .daemon
        .client()
        .platform_search("")
        .await
        .map_err(to_http)?;
    Ok(Json(json!({ "platforms": platforms })))
}

/// `GET /api/platforms/{id}` — one platform's status by exact id
/// (`vendor:architecture`, e.g. `esp32:esp32`), or 404 when the indexes don't
/// know it. The full search is filtered here because arduino-cli's
/// `search_args` is fuzzy, and the local indexes make the full list cheap.
pub(crate) async fn platform_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let platforms = state
        .daemon
        .client()
        .platform_search("")
        .await
        .map_err(to_http)?;

    match platforms.into_iter().find(|platform| platform.id == id) {
        Some(platform) => Ok(Json(
            serde_json::to_value(platform).expect("PlatformStatus always serializes"),
        )),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": "invalidRequest",
                "message": format!("unknown platform: {id}"),
            })),
        )),
    }
}

/// Map a helper [`Error`] onto an HTTP status plus the `{code, message}` body.
/// Anything that isn't the caller's fault is the daemon's, hence 502.
fn to_http(error: Error) -> ApiError {
    let status = match error {
        Error::InvalidRequest(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    (
        status,
        Json(json!({ "code": error.code(), "message": error.to_string() })),
    )
}
