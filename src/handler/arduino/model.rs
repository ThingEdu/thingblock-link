//! Arduino data types the handler produces and consumes. Shared by every gateway's wire model,
//! so none of them belongs to one transport.

use serde::{Deserialize, Serialize};

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

/// Install status of a boards platform, returned by the HTTP `/api/platforms` routes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
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
