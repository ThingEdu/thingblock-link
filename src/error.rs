use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("daemon: {0}")]
    Daemon(String),

    #[error("grpc: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("cancelled")]
    Cancelled,

    #[error("resource: {0}")]
    Resource(String),

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

/// Same `{code, message}` body as the WS `error`, so HTTP callers parse one error shape.
impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;

        let status = match self {
            // The root is validated at startup, so a resource error here is a bad caller reference.
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
    /// Stable wire code for the WS `error {code, message}` message.
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
