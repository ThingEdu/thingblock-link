//! Serde structs for the WS `{id, type, payload}` envelope — the single contract
//! between this helper and the browser/editor (see the design doc). The
//! arduino-cli gRPC schema never appears here, keeping the daemon swappable and
//! the JS side ignorant of arduino-cli specifics.
//!
//! `id` correlates a request with its streamed responses and its one terminal
//! reply. Wire field names are camelCase to match the JS side.

use serde::{Deserialize, Serialize};

/// A message from the browser to the helper.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub body: RequestBody,
}

/// Client → helper message bodies, discriminated by `type` with the variant data
/// carried under `payload` (adjacently tagged).
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RequestBody {
    ListBoards {
        pnpid: Vec<String>,
    },
    Connect {
        port: String,
    },
    Disconnect {},
    Compile {
        fqbn: String,
        options: serde_json::Value,
        source: String,
        /// References to vendored library directories under the served resource
        /// root, resolved in place by the daemon (no bytes cross the WS). Absent
        /// for clients that don't vendor libs, hence `default`.
        #[serde(default)]
        libs: Vec<LibRef>,
    },
    Upload {
        fqbn: String,
        port: String,
        upload_speed: u32,
        artifact: Artifact,
    },
    MonitorOpen {
        port: String,
        baud_rate: u32,
    },
    MonitorWrite {
        data: String,
    },
    MonitorClose {},
    /// Install a boards platform (core) by id, e.g. `esp32:esp32`, downloading
    /// it via the board manager. Streams `progress`/`log`; cancellable. Serialized
    /// daemon-wide — a second install while one runs is rejected.
    InstallPlatform {
        platform: String,
        /// Specific version to install; absent means the latest indexed release.
        #[serde(default)]
        version: Option<String>,
    },
    /// Targets an in-flight request `id`; drops its underlying tonic stream.
    Cancel {},
}

/// A message from the helper to the browser.
#[derive(Debug, Serialize)]
pub struct Response {
    pub id: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

/// Helper → client message bodies. Streamed (`log`, `progress`, `monitorData`),
/// terminal (`result`, `error`), or unsolicited (`event`).
///
/// `result` and `event` payloads vary per request, so they carry a free-form
/// `Value`; the typed helper structs below serialize into it.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResponseBody {
    /// Streamed stdout/stderr chunk for a request `id`.
    Log { chunk: String },
    /// Streamed progress for a request `id`.
    Progress { phase: String, percent: f32 },
    /// Terminal success for a request `id`.
    Result(serde_json::Value),
    /// Terminal failure for a request `id`.
    Error { code: String, message: String },
    /// Inbound serial bytes for the monitor session.
    MonitorData { data: String },
    /// Unsolicited event, e.g. `boardListChanged` from `BoardListWatch`.
    Event(serde_json::Value),
}

/// A reference to a vendored library *directory* inside an installed pack under
/// the served resource root. `lib` is a directory path relative to the pack; the
/// helper resolves it to a local path the arduino-cli daemon reads in place. No
/// version is carried — the root is single-version (see the resource-serving doc).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibRef {
    pub pack: String,
    pub lib: String,
}

/// A compiled binary the editor can hand back to `upload`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub format: String,
    pub path: String,
    /// Base64 firmware bytes, set by the cloud server whose caller is a browser
    /// with no access to `path`. Absent for the local helper, which uploads the
    /// file in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The separately-addressed images an ESP flash needs (bootloader, partition
    /// table, boot_app0, app). Empty for single-image targets such as AVR, whose
    /// Intel HEX carries its own addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ArtifactPart>,
}

/// One image within an [`Artifact`], flashed at a fixed offset.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPart {
    /// Flash offset, e.g. `0x10000` for the app image.
    pub offset: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Base64 bytes, filled in by the cloud server (see [`Artifact::data`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Install status of a boards platform (a core such as `esp32:esp32`), as
/// returned by the HTTP `GET /api/platforms` routes. HTTP rather than WS
/// because it is a one-shot read — but it is the same helper↔editor contract,
/// so it lives here with the envelope types.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformStatus {
    /// Platform id, `vendor:architecture` (e.g. `esp32:esp32`).
    pub id: String,
    /// Human-readable name (e.g. "Arduino AVR Boards").
    pub name: String,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
}

/// A connectable board, as returned by `listBoards`. Shape is opaque to the JS
/// `Connection` contract beyond the fields it reads.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTarget {
    pub port: String,
    pub label: String,
}

/// `result` payload for `listBoards`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListBoardsResult {
    pub targets: Vec<ConnectionTarget>,
}

/// `result` payload for `compile`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub artifact: Artifact,
}

/// The `compile` request's `options` payload — a deliberately tolerant subset of
/// what the editor may send. Every field defaults and unknown keys are ignored,
/// so an unexpected shape from the JS side degrades to a plain compile rather
/// than a hard error. The exact contract with the firmware module's
/// `getUploadConfig()` is still to be pinned down (see the design doc).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompileOptions {
    /// Turn on arduino-cli verbose compile output.
    pub verbose: bool,
    /// gcc warning level: "none", "default", "more", "all".
    pub warnings: Option<String>,
    /// Paths to single library root directories.
    pub libraries: Vec<String>,
    /// Custom `key=value` build properties.
    pub build_properties: Vec<String>,
}
