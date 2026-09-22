//! Serde structs for the `/io` `{id, type, payload}` envelope (camelCase, like the JS side).
//! Parallel to, never shared with, the flash envelope so each channel stays removable.

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

/// Browser → helper bodies: `type` picks the variant and `payload` carries its data
/// (adjacently tagged, matching the flash envelope).
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
    /// Targets an in-flight request `id`; nothing is long-running yet, but the arm keeps the
    /// wire contract stable.
    Cancel {},
}

/// A message from the helper to the browser over `/io`.
#[derive(Debug, Serialize)]
pub struct BleResponse {
    pub id: String,
    #[serde(flatten)]
    pub body: BleResponseBody,
}

/// Helper → browser bodies. `Device`/`Notify`/`Disconnected` are the M2 wire contract
/// (scan hits, notifications, unsolicited disconnects); unused until then.
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

/// Sends one request's responses to the session's writer task, stamped with the request `id`.
/// Separate from the flash channel's `Responder` so `/io` doesn't depend on its types.
#[derive(Clone)]
pub struct Responder {
    id: String,
    tx: mpsc::Sender<BleResponse>,
}

impl Responder {
    pub fn new(id: String, tx: mpsc::Sender<BleResponse>) -> Self {
        Self { id, tx }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Sends one response body for this request. A closed channel (browser gone or writer
    /// ended) isn't actionable here, so it is logged and dropped.
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
