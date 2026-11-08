//! Crate-wide error type, one variant per domain, each carrying a message naming the
//! offending input (path, port, id, ...).

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// A malformed envelope from the browser, or the gateway failed to bind or serve.
    #[error("gateway: {0}")]
    Gateway(String),

    /// A request was cancelled via `cancel{id}`.
    #[error("cancelled")]
    Cancelled,

    /// The arduino-cli daemon failed to start, its gRPC channel dropped, or an RPC failed.
    #[error("arduino: {0}")]
    Arduino(String),

    /// The BLE adapter errored, a peripheral wasn't in the scan cache, or a requested
    /// characteristic doesn't exist on the connected device.
    #[error("ble: {0}")]
    Ble(String),

    /// The resource root is invalid, or a referenced pack/lib/file is missing or escapes it.
    #[error("thingblock: {0}")]
    Thingblock(String),
}

impl From<tonic::Status> for Error {
    fn from(status: tonic::Status) -> Self {
        Error::Arduino(format!("{:?}: {}", status.code(), status.message()))
    }
}

impl From<btleplug::Error> for Error {
    fn from(e: btleplug::Error) -> Self {
        Error::Ble(e.to_string())
    }
}
