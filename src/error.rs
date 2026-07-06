//! Crate-wide error type.
//!
//! Errors originate at the two boundaries we validate (the WS envelope from the
//! browser and the arduino-cli daemon's responses) and are carried back to the
//! editor as the WS `error {code, message}` terminal message. Internal code
//! trusts its inputs (see AGENTS.md), so this stays deliberately small.

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

    /// A resource-root or pack/lib reference problem: the configured root is
    /// invalid, or a referenced pack/lib is missing or escapes the root. Carries
    /// an actionable message (the offending root, pack, or lib).
    #[error("resource: {0}")]
    Resource(String),

    /// A BLE operation failed: the adapter subsystem errored, a peripheral
    /// wasn't in the scan cache, or a requested characteristic doesn't exist
    /// on the connected device.
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
