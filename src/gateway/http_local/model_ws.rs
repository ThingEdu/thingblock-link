//! Serde types for the WS `{id, type, payload}` envelope exchanged with the editor.

use serde::{Deserialize, Serialize};

use crate::service::arduino::grpc::platform::Platform;
use crate::service::arduino::model::{Artifact, CompileOptions};

/// A browser → helper message. `id` correlates it with its streamed responses and terminal reply.
#[derive(Debug, Deserialize)]
pub struct WsRequest {
    pub id: String,
    #[serde(flatten)]
    pub body: WsRequestBody,
}

/// Request bodies, discriminated by `type` with the variant data under `payload`.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WsRequestBody {
    /// Binds the session to `port` once it is detected as one of the `pnpid` boards.
    Connect {
        port: String,
        pnpid: Vec<String>,
    },
    /// Closes the monitor, cancels every in-flight request and clears the connected port.
    Disconnect {},
    Compile {
        fqbn: String,
        options: CompileOptions,
        source: String,
        /// Vendored lib dirs under the resource root; clients that don't vendor libs omit it.
        #[serde(default)]
        libs: Vec<LibRef>,
    },
    /// Uploads to the connected port.
    Upload {
        fqbn: String,
        upload_speed: u32,
        artifact: Artifact,
    },
    /// Flashes a prebuilt image shipped in a pack, resolved under the resource root, to the
    /// connected port.
    FlashFirmware {
        fqbn: String,
        upload_speed: u32,
        pack: String,
        file: String,
    },
    /// Opens the serial monitor on the connected port.
    MonitorOpen {
        baud_rate: u32,
    },
    MonitorWrite {
        data: String,
    },
    MonitorClose {},
    /// Installs a supported boards platform at its pinned version.
    InstallPlatform {
        platform: Platform,
    },
    /// Cancels the in-flight request whose id equals this envelope's `id`.
    Cancel {},
}

/// A vendored library directory, `lib` relative to its `pack` under the resource root.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibRef {
    pub pack: String,
    pub lib: String,
}

/// A helper → browser message, correlated to its request by `id`.
#[derive(Debug, Serialize)]
pub struct WsResponse {
    pub id: String,
    #[serde(flatten)]
    pub body: WsResponseBody,
}

/// Response bodies: streamed (`log`, `progress`, `monitorData`) or terminal (`error`, and every
/// `result` variant). Serialize-only, so several variants can share the `result` tag.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WsResponseBody {
    Log {
        chunk: String,
    },
    Progress {
        phase: String,
        percent: f32,
    },
    MonitorData {
        data: String,
    },
    Error {
        code: String,
        message: String,
    },

    /// `{}` for requests with nothing to return (connect, upload, monitorOpen, …).
    #[serde(rename = "result")]
    Ok {},
    #[serde(rename = "result")]
    Compile {
        artifact: Artifact,
    },
}
