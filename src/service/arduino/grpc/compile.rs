//! `Compile` translation: drive arduino-cli's `Compile` server-stream and map its
//! `CompileResponse` oneof onto helper-shaped [`CompileEvent`]s. Backs the WS
//! `compile` request.
//!
//! arduino-cli compiles a sketch *directory* (not raw source), so the caller
//! materializes the source first and passes the path here. The final
//! `BuilderResult` carries only the build directory; we locate the flashable
//! binary within it ([`find_artifact`]). The arduino-cli schema never leaks past
//! this module — the bridge sees only [`CompileEvent`].

use std::path::{Path, PathBuf};

use futures::Stream;
use tonic::Streaming;
use tracing::warn;

use crate::error::{Error, Result};
use crate::server::protocol::{Artifact, ArtifactPart, CompileOptions};
use crate::service::arduino::grpc::{Client, cli};

/// One translated step of a compile, in the helper's own shapes.
#[derive(Debug)]
pub enum CompileEvent {
    /// A stdout/stderr chunk from the compiler.
    Log(String),
    /// Compiler progress.
    Progress { phase: String, percent: f32 },
    /// Terminal success: the located build artifact.
    Done(Artifact),
}

impl Client {
    /// Compile the sketch at `sketch_path` for `fqbn`, streaming translated
    /// events. The stream ends after a `Done` (success) or yields a single `Err`
    /// (gRPC status / no artifact) and then ends.
    pub async fn compile(
        &mut self,
        fqbn: &str,
        sketch_path: &Path,
        opts: &CompileOptions,
        lib_dirs: &[PathBuf],
    ) -> Result<impl Stream<Item = Result<CompileEvent>>> {
        let instance = *self.instance();
        // `library` (one entry per single-library root dir) carries both the
        // editor-supplied paths and the resource-resolved vendored lib dirs.
        let library = opts
            .libraries
            .iter()
            .cloned()
            .chain(lib_dirs.iter().map(|d| d.to_string_lossy().into_owned()))
            .collect();
        let request = cli::CompileRequest {
            instance: Some(instance),
            fqbn: fqbn.to_string(),
            sketch_path: sketch_path.to_string_lossy().into_owned(),
            verbose: opts.verbose,
            warnings: opts.warnings.clone().unwrap_or_default(),
            library,
            build_properties: opts.build_properties.clone(),
            ..Default::default()
        };

        let stream = self.inner().compile(request).await?.into_inner();
        Ok(into_events(stream))
    }
}

/// Adapt the tonic `Compile` stream into a `CompileEvent` stream, skipping empty
/// frames and terminating after the first error.
fn into_events(
    stream: Streaming<cli::CompileResponse>,
) -> impl Stream<Item = Result<CompileEvent>> {
    futures::stream::unfold((stream, false), |(mut stream, done)| async move {
        if done {
            return None;
        }
        loop {
            match stream.message().await {
                Ok(Some(resp)) => {
                    if let Some(event) = translate(resp) {
                        let stop = event.is_err();
                        return Some((event, (stream, stop)));
                    }
                    // Empty oneof — nothing to surface; keep reading.
                }
                Ok(None) => return None, // stream ended cleanly
                Err(status) => return Some((Err(Error::Grpc(status)), (stream, true))),
            }
        }
    })
}

/// Map one `CompileResponse` to a `CompileEvent`, or `None` for an empty frame.
fn translate(resp: cli::CompileResponse) -> Option<Result<CompileEvent>> {
    use cli::compile_response::Message;

    match resp.message? {
        Message::OutStream(bytes) | Message::ErrStream(bytes) => Some(Ok(CompileEvent::Log(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Message::Progress(progress) => Some(Ok(CompileEvent::Progress {
            // `name` is the task label; fall back to the freeform `message`.
            phase: if progress.name.is_empty() {
                progress.message
            } else {
                progress.name
            },
            percent: progress.percent,
        })),
        Message::Result(result) => {
            match find_artifact(Path::new(&result.build_path), &result.build_properties) {
                Some(artifact) => Some(Ok(CompileEvent::Done(artifact))),
                None => Some(Err(Error::Daemon(format!(
                    "compile produced no flashable artifact in {}",
                    result.build_path
                )))),
            }
        }
    }
}

/// Locate the flashable binary in a build directory, preferring an AVR `.ino.hex`
/// then an ESP `.ino.bin`. Pure (filesystem-only, no daemon) so it is unit
/// testable without hardware.
///
/// The `.ino.<ext>` suffix naturally skips merged variants such as
/// `*.ino.with_bootloader.hex`, which we don't flash directly.
pub fn find_artifact(build_path: &Path, build_properties: &[String]) -> Option<Artifact> {
    for ext in ["hex", "bin"] {
        if let Some(path) = find_binary(build_path, ext) {
            let parts = if ext == "bin" {
                esp_parts(build_path, &path, build_properties)
            } else {
                Vec::new()
            };
            return Some(Artifact {
                format: ext.to_string(),
                path: path.to_string_lossy().into_owned(),
                data: None,
                parts,
            });
        }
    }
    None
}

/// The four images an ESP flash needs, at the offsets the core's own `merge-bin`
/// recipe uses. Only the bootloader's offset varies by chip (`0x0` on RISC-V
/// parts, `0x1000` on the classic ESP32), so it comes from the build properties
/// rather than a table here; a build missing any image yields no parts at all,
/// leaving the single-image path intact.
fn esp_parts(build_path: &Path, app: &Path, build_properties: &[String]) -> Vec<ArtifactPart> {
    esp_parts_inner(build_path, app, build_properties).unwrap_or_default()
}

fn esp_parts_inner(
    build_path: &Path,
    app: &Path,
    build_properties: &[String],
) -> Option<Vec<ArtifactPart>> {
    let stem = app
        .file_name()
        .and_then(|n| n.to_str())?
        .trim_end_matches(".bin");
    let bootloader_addr = build_properties
        .iter()
        .find_map(|p| p.strip_prefix("build.bootloader_addr="))
        .and_then(|v| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())?;

    let images = [
        (
            bootloader_addr,
            build_path.join(format!("{stem}.bootloader.bin")),
        ),
        (0x8000, build_path.join(format!("{stem}.partitions.bin"))),
        (0xe000, build_path.join("boot_app0.bin")),
        (0x10000, app.to_path_buf()),
    ];
    if images.iter().any(|(_, path)| !path.is_file()) {
        warn!(build = %build_path.display(), "esp build is missing an image; flashing the app alone");
        return None;
    }
    Some(
        images
            .into_iter()
            .map(|(offset, path)| ArtifactPart {
                offset,
                path: path.to_string_lossy().into_owned(),
                data: None,
            })
            .collect(),
    )
}

/// First file in `dir` whose name ends with `.ino.<ext>`.
fn find_binary(dir: &Path, ext: &str) -> Option<PathBuf> {
    let suffix = format!(".ino.{ext}");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        name.to_string_lossy()
            .ends_with(&suffix)
            .then(|| entry.path())
    })
}
