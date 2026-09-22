//! Deliberately parallel to, never shared with, the flash envelope so each channel is removable.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug, Deserialize)]
pub struct BleRequest {
    pub id: String,
    #[serde(flatten)]
    pub body: BleRequestBody,
}

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
    /// Nothing long-running to cancel yet; the arm keeps the wire contract stable.
    Cancel {},
}

#[derive(Debug, Serialize)]
pub struct BleResponse {
    pub id: String,
    #[serde(flatten)]
    pub body: BleResponseBody,
}

/// `Device`/`Notify`/`Disconnected` are M2 wire contract, unused until then.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BleResponseBody {
    Result(serde_json::Value),
    Error {
        code: String,
        message: String,
    },
    #[serde(rename = "bleDevice")]
    Device {
        device_id: String,
        name: Option<String>,
        rssi: Option<i16>,
    },
    #[serde(rename = "bleNotify")]
    Notify {
        device_id: String,
        characteristic: String,
        data: String,
    },
    /// Unsolicited: sent without a matching client request.
    #[serde(rename = "bleDisconnected")]
    Disconnected {
        device_id: String,
    },
}

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

    /// A closed channel (browser gone) isn't actionable here, so the response is dropped.
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
