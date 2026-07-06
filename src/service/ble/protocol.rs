//! Serde structs for the `/io` channel's `{id, type, payload}` envelope.
//!
//! Deliberately parallel to (never re-exported from) [`crate::server::protocol`]:
//! the `/io` channel talks BLE, the flash channel talks arduino-cli, and the
//! two must stay swappable/removable independently of each other. Wire field
//! names are camelCase to match the JS side, same as the flash envelope.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;

/// A message from the browser to the helper over `/io`.
#[derive(Debug, Deserialize)]
pub struct BleRequest {
    pub id: String,
    #[serde(flatten)]
    pub body: BleRequestBody,
}

/// Client → helper message bodies, discriminated by `type` with the variant
/// data carried under `payload` (adjacently tagged, matching the flash
/// envelope's convention).
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BleRequestBody {
    Scan {
        services: Vec<String>,
        #[serde(default)]
        name_prefix: Option<String>,
    },
    Connect {
        device_id: String,
    },
    Disconnect {
        device_id: String,
    },
    Write {
        device_id: String,
        service: String,
        characteristic: String,
        data: String,
        #[serde(default)]
        with_response: bool,
    },
    Read {
        device_id: String,
        service: String,
        characteristic: String,
    },
    Subscribe {
        device_id: String,
        service: String,
        characteristic: String,
    },
    Unsubscribe {
        device_id: String,
        service: String,
        characteristic: String,
    },
    /// Targets an in-flight request `id`; M1 has no long-running requests to
    /// cancel, but the arm exists so the wire contract is stable from M1.
    Cancel {},
}

/// A message from the helper to the browser over `/io`.
#[derive(Debug, Serialize)]
pub struct BleResponse {
    pub id: String,
    #[serde(flatten)]
    pub body: BleResponseBody,
}

/// Helper → client message bodies for `/io`. `Device`/`Notify`/
/// `Disconnected` are the M2 wire contract (scan hits, notifications, and
/// unsolicited disconnects); unused until then.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BleResponseBody {
    /// Terminal success for a request `id`.
    Result(serde_json::Value),
    /// Terminal failure for a request `id`.
    Error { code: String, message: String },
    /// A scan hit for a request `id`'s in-progress `scan`.
    #[serde(rename = "bleDevice")]
    Device {
        device_id: String,
        name: Option<String>,
        rssi: Option<i16>,
    },
    /// An inbound notification for a subscribed characteristic.
    #[serde(rename = "bleNotify")]
    Notify {
        device_id: String,
        characteristic: String,
        data: String,
    },
    /// Unsolicited: a connected peripheral dropped its connection.
    #[serde(rename = "bleDisconnected")]
    Disconnected { device_id: String },
}

/// Sends responses for one request back to the session's writer task,
/// stamping each with the request `id`. Deliberately not [`crate::service::arduino::bridge::Responder`]
/// — this channel owns its own envelope and must not depend on the flash
/// channel's types.
#[derive(Clone)]
pub struct Responder {
    id: String,
    tx: mpsc::Sender<BleResponse>,
}

impl Responder {
    pub fn new(id: String, tx: mpsc::Sender<BleResponse>) -> Self {
        Self { id, tx }
    }

    /// The request `id` this responder stamps onto every reply, for log context.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Send one response body for this request. A closed channel (browser gone
    /// or writer task ended) is not actionable here, so it is logged and dropped.
    pub async fn send(&self, body: BleResponseBody) {
        let response = BleResponse {
            id: self.id.clone(),
            body,
        };
        if self.tx.send(response).await.is_err() {
            warn!(id = %self.id, "io response dropped: ws writer closed");
        }
    }
}
