//! WS `{id, type, payload}` envelope: the cross-repo contract with the editor; keep gRPC types out.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub body: RequestBody,
}

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
        #[serde(default)]
        libs: Vec<LibRef>,
    },
    Upload {
        fqbn: String,
        port: String,
        upload_speed: u32,
        artifact: Artifact,
    },
    /// `pack`/`file` resolve under the resource root: the browser can't name a helper filesystem path.
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
    /// Serialized daemon-wide: a second install while one runs is rejected.
    InstallPlatform {
        platform: String,
        /// Absent means the latest indexed release.
        #[serde(default)]
        version: Option<String>,
    },
    /// Targets the in-flight request with the same `id`.
    Cancel {},
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub id: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResponseBody {
    Log { chunk: String },
    Progress { phase: String, percent: f32 },
    Result(serde_json::Value),
    Error { code: String, message: String },
    MonitorData { data: String },
    Event(serde_json::Value),
}

/// `lib` is a directory relative to the pack; no version since the resource root is single-version.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibRef {
    pub pack: String,
    pub lib: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub format: String,
    pub path: String,
    /// Base64 bytes, set only by the cloud server since its browser caller can't read `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// ESP multi-image flash (bootloader, partitions, boot_app0, app); empty for AVR's self-addressed HEX.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ArtifactPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPart {
    pub offset: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformStatus {
    /// `vendor:architecture`, e.g. `esp32:esp32`.
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTarget {
    pub port: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListBoardsResult {
    pub targets: Vec<ConnectionTarget>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub artifact: Artifact,
}

/// Tolerant by design: every field defaults and unknown keys are ignored, degrading to a plain compile.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompileOptions {
    pub verbose: bool,
    /// gcc warning level: "none", "default", "more", "all".
    pub warnings: Option<String>,
    pub libraries: Vec<String>,
    pub build_properties: Vec<String>,
}
