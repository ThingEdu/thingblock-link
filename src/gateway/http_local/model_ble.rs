//! Serde types for the `/ws/ble` `{id, type, payload}` envelope exchanged with the editor.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A browser → helper message. `id` correlates it with its streamed responses and terminal reply.
#[derive(Debug, Deserialize)]
pub struct BleRequest {
    pub id: String,
    #[serde(flatten)]
    pub body: BleRequestBody,
}

/// Request bodies, discriminated by `type` with the variant data under `payload`.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BleRequestBody {
    /// Streams `device` hits advertising any of `services` (every device if empty) until
    /// cancelled.
    Scan {
        services: Vec<Uuid>,
        #[serde(default)]
        name_prefix: Option<String>,
    },
    /// Connects a device found by a scan.
    Connect {
        device_id: String,
    },
    Disconnect {
        device_id: String,
    },
    /// Writes base64 `data` to a characteristic.
    Write {
        device_id: String,
        service: Uuid,
        characteristic: Uuid,
        data: String,
        #[serde(default)]
        with_response: bool,
    },
    Read {
        device_id: String,
        service: Uuid,
        characteristic: Uuid,
    },
    /// Enables `notify` messages for a characteristic.
    Subscribe {
        device_id: String,
        service: Uuid,
        characteristic: Uuid,
    },
    Unsubscribe {
        device_id: String,
        service: Uuid,
        characteristic: Uuid,
    },
    /// Stops the scan whose id equals this envelope's `id`.
    Cancel {},
}

/// A helper → browser message, correlated to its request by `id`; empty for unsolicited ones.
#[derive(Debug, Serialize)]
pub struct BleResponse {
    pub id: String,
    #[serde(flatten)]
    pub body: BleResponseBody,
}

/// Response bodies: streamed (`device`), unsolicited (`notify`, `disconnected`) or terminal
/// (`error`, and every `result` variant). Serialize-only, so several variants can share the
/// `result` tag.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BleResponseBody {
    /// A scan hit, under the `scan` request's id.
    Device {
        device_id: String,
        name: Option<String>,
        rssi: Option<i16>,
    },
    /// A notification from a subscribed characteristic, `data` in base64.
    Notify {
        device_id: String,
        characteristic: String,
        data: String,
    },
    /// A connected device dropped its connection.
    Disconnected {
        device_id: String,
    },
    Error {
        code: String,
        message: String,
    },

    /// `{}` for requests with nothing to return.
    #[serde(rename = "result")]
    Ok {},
    #[serde(rename = "result")]
    Connect {
        device_id: String,
    },
    /// The characteristic value in base64.
    #[serde(rename = "result")]
    Read {
        data: String,
    },
}
