use serde::Serialize;

use crate::handler::arduino::model::{ConnectionTarget, PlatformInfo};

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

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum HttpResponseBody {
    Error { code: String, message: String },

    ArduinoListPlatforms { platforms: Vec<PlatformInfo> },
    ArduinoListBoards { targets: Vec<ConnectionTarget> },
}
