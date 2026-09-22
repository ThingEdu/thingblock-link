//! Serde types for the WS `{id, type, payload}` envelope, the one contract with the editor. No
//! arduino-cli gRPC types appear here, so the JS side stays ignorant of the daemon.

use serde::{Deserialize, Serialize};

/// A browser → helper message. `id` correlates it with its streamed responses and terminal reply.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub body: RequestBody,
}

/// Request bodies, discriminated by `type` with the variant data under `payload`.
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
        /// Vendored lib dirs under the resource root, read in place (no bytes cross the WS).
        /// Defaulted because clients that don't vendor libs omit it.
        #[serde(default)]
        libs: Vec<LibRef>,
    },
    Upload {
        fqbn: String,
        port: String,
        upload_speed: u32,
        artifact: Artifact,
    },
    /// Flashes a prebuilt image a device pack ships, skipping compile. `pack`/`file` resolve under
    /// the resource root since the browser can't name a helper filesystem path.
    FlashFirmware {
        fqbn: String,
        port: String,
        upload_speed: u32,
        pack: String,
        file: String,
    },
    MonitorOpen {
        port: String,
        baud_rate: u32,
    },
    MonitorWrite {
        data: String,
    },
    MonitorClose {},
    /// Installs a boards platform (e.g. `esp32:esp32`), streaming `progress`/`log`; cancellable.
    /// Serialized daemon-wide: a second install while one runs is rejected.
    InstallPlatform {
        platform: String,
        /// Absent means the latest indexed release.
        #[serde(default)]
        version: Option<String>,
    },
    /// Cancels the in-flight request with the same `id` by dropping its tonic stream.
    Cancel {},
}

/// A helper → browser message, correlated to its request by `id`.
#[derive(Debug, Serialize)]
pub struct Response {
    pub id: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

/// Response bodies: streamed (`log`, `progress`, `monitorData`), terminal (`result`, `error`), or
/// unsolicited (`event`). `result`/`event` payloads vary per request, hence a free-form `Value`.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResponseBody {
    Log {
        chunk: String,
    },
    Progress {
        phase: String,
        percent: f32,
    },
    Result(serde_json::Value),
    Error {
        code: String,
        message: String,
    },
    MonitorData {
        data: String,
    },
    /// Unsolicited, e.g. `boardListChanged` from `BoardListWatch`.
    Event(serde_json::Value),
}

/// A vendored library directory inside a pack under the resource root. `lib` is relative to the
/// pack; no version is carried because the resource root is single-version.
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
    /// Base64 bytes, set only by the cloud server since its browser caller can't read `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Separately addressed images an ESP flash needs (bootloader, partitions, boot_app0, app).
    /// Empty for single-image targets like AVR, whose Intel HEX carries its own addresses.
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

/// Install status of a boards platform, returned by the HTTP `/api/platforms` routes. Lives here
/// because it's part of the same helper↔editor contract, even though it goes over HTTP.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformStatus {
    /// `vendor:architecture`, e.g. `esp32:esp32`.
    pub id: String,
    /// Human-readable name, e.g. "Arduino AVR Boards".
    pub name: String,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
}

/// A connectable board returned by `listBoards`.
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

/// The `compile` request's `options`, tolerant by design: every field defaults and unknown keys
/// are ignored, so an unexpected shape from the editor degrades to a plain compile.
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
