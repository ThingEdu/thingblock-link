use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;

use crate::error::Error;
use crate::server::router::AppState;

/// Body reuses the WS terminal error's `{code, message}` shape.
type ApiError = (StatusCode, Json<serde_json::Value>);

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
