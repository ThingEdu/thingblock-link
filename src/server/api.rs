//! HTTP JSON API for one-shot reads (platform install status for the editor's board manager).
//! Plain HTTP fits a one-shot read better than the WS envelope; streaming installs stay on the WS.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;

use crate::error::Error;
use crate::server::router::AppState;

/// Error response; the body reuses the WS terminal error's `{code, message}` shape.
type ApiError = (StatusCode, Json<serde_json::Value>);

/// `GET /api/platforms`: every indexed platform with its install status.
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

/// `GET /api/platforms/{id}`: one platform's status by exact id (e.g. `esp32:esp32`), or 404.
/// Filters the full search locally because arduino-cli's `search_args` is fuzzy, not exact.
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

/// Maps a helper [`Error`] to an HTTP status plus `{code, message}` body. Anything that isn't
/// the caller's fault is the daemon's, hence 502.
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
