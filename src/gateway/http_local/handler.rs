//! Axum handlers for the local gateway's HTTP routes: each calls a service and maps its result
//! and its own failure cases to an HTTP status and JSON body.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::warn;

use crate::gateway::GatewayContext;
use crate::gateway::http_local::model::{HttpResponse, HttpResponseBody};

type HttpError = (StatusCode, HttpResponse);

/// `GET /api/platforms`: every indexed platform with its install status.
pub async fn list_platforms(
    State(context): State<GatewayContext>,
) -> Result<HttpResponse, HttpError> {
    let platforms = context
        .arduino
        .client()
        .platform_search("")
        .await
        .map_err(|e| error(StatusCode::BAD_GATEWAY, "daemonError", e))?;
    Ok(HttpResponse::new(HttpResponseBody::ArduinoListPlatforms {
        platforms,
    }))
}

/// `GET /api/platforms/{id}`: one platform's install status by exact id (e.g. `esp32:esp32`).
pub async fn platform_status(
    State(context): State<GatewayContext>,
    Path(id): Path<String>,
) -> Result<HttpResponse, HttpError> {
    let platforms = context
        .arduino
        .client()
        .platform_search("")
        .await
        .map_err(|e| error(StatusCode::BAD_GATEWAY, "daemonError", e))?;

    // Exact match on the full list: arduino-cli's search query is fuzzy.
    match platforms.into_iter().find(|platform| platform.id == id) {
        Some(platform) => Ok(HttpResponse::new(HttpResponseBody::ArduinoPlatform(
            platform,
        ))),
        None => Err(error(
            StatusCode::NOT_FOUND,
            "notFound",
            format!("unknown platform: {id}"),
        )),
    }
}

/// Query of `GET /api/boards`: `pnpid` is a comma-separated list of Windows PNP ids.
#[derive(Deserialize)]
pub struct BoardsQuery {
    pnpid: Option<String>,
}

/// `GET /api/boards?pnpid=...`: connected boards whose USB pnpid is in the list.
pub async fn list_boards(
    State(context): State<GatewayContext>,
    Query(query): Query<BoardsQuery>,
) -> Result<HttpResponse, HttpError> {
    let pnpid: Vec<String> = query
        .pnpid
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(String::from)
        .collect();
    if pnpid.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalidRequest",
            "pnpid is required",
        ));
    }

    let targets = context
        .arduino
        .client()
        .board_list(&pnpid)
        .await
        .map_err(|e| error(StatusCode::BAD_GATEWAY, "daemonError", e))?;
    Ok(HttpResponse::new(HttpResponseBody::ArduinoListBoards {
        targets,
    }))
}

/// Builds an error response with a `{code, message}` body, logging it.
fn error(status: StatusCode, code: &str, message: impl std::fmt::Display) -> HttpError {
    let message = message.to_string();
    warn!(%status, code, %message, "http request failed");
    (
        status,
        HttpResponse::new(HttpResponseBody::Error {
            code: code.to_string(),
            message,
        }),
    )
}
