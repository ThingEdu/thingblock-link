use axum::Json;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::service::arduino::model::{ConnectionTarget, PlatformInfo};

pub struct HttpRequest {
    body: HttpRequestBody,
}

pub enum HttpRequestBody {}

/// A route's JSON response, serialized as just the body.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct HttpResponse {
    body: HttpResponseBody,
}

impl HttpResponse {
    pub fn new(body: HttpResponseBody) -> Self {
        Self { body }
    }
}

impl IntoResponse for HttpResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum HttpResponseBody {
    Error { code: String, message: String },

    ArduinoListPlatforms { platforms: Vec<PlatformInfo> },
    ArduinoPlatform(PlatformInfo),
    ArduinoListBoards { targets: Vec<ConnectionTarget> },
}
