//! Crate-wide error type. Errors arise at the validated boundaries (browser envelope, daemon
//! responses) and reach the editor as the WS `error {code, message}` terminal message.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// A malformed or unexpected WS envelope arrived from the browser.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The arduino-cli daemon failed to start, or its gRPC channel dropped.
    #[error("daemon: {0}")]
    Daemon(String),

    /// A gRPC call to the daemon returned an error status.
    #[error("grpc: {0}")]
    Grpc(#[from] tonic::Status),

    /// A request was cancelled via `cancel{id}`.
    #[error("cancelled")]
    Cancelled,

    /// The configured resource root is invalid, or a referenced pack/lib/file is missing or
    /// escapes it; the message names the offender.
    #[error("resource: {0}")]
    Resource(String),

    /// The BLE adapter errored, a peripheral wasn't in the scan cache, or a requested
    /// characteristic doesn't exist on the connected device.
    #[error("ble: {0}")]
    Ble(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<btleplug::Error> for Error {
    fn from(e: btleplug::Error) -> Self {
        Error::Ble(e.to_string())
    }
}

/// Renders the same `{code, message}` body as the WS terminal `error`, so the cloud server's
/// HTTP callers parse one error shape across both faces.
impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;

        let status = match self {
            // The root is validated at startup, so a resource error here is a bad
            // `{pack, lib}` reference from the caller.
            Error::InvalidRequest(_) | Error::Resource(_) => StatusCode::BAD_REQUEST,
            Error::Cancelled => StatusCode::from_u16(499).expect("valid status"),
            Error::Daemon(_) | Error::Grpc(_) => StatusCode::BAD_GATEWAY,
            Error::Ble(_) | Error::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = axum::Json(serde_json::json!({
            "code": self.code(),
            "message": self.to_string(),
        }));
        (status, body).into_response()
    }
}

impl Error {
    /// Stable wire code for the WS `error {code, message}` terminal message.
    pub fn code(&self) -> &'static str {
        match self {
            Error::InvalidRequest(_) => "invalidRequest",
            Error::Daemon(_) => "daemon",
            Error::Grpc(_) => "grpc",
            Error::Cancelled => "cancelled",
            Error::Resource(_) => "resource",
            Error::Ble(_) => "ble",
            Error::Io(_) => "io",
        }
    }
}
