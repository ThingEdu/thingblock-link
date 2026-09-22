use std::path::{Path, PathBuf};

use futures::Stream;
use tonic::Streaming;
use tracing::warn;

use crate::error::{Error, Result};
use crate::server::protocol::{Artifact, ArtifactPart, CompileOptions};
use crate::service::arduino::grpc::{Client, cli};

#[derive(Debug)]
pub enum CompileEvent {
    Log(String),
    Progress { phase: String, percent: f32 },
    Done(Artifact),
}

impl Client {
    /// `sketch_path` must be a sketch directory; arduino-cli doesn't compile raw source.
    pub async fn compile(
        &mut self,
        fqbn: &str,
        sketch_path: &Path,
        opts: &CompileOptions,
        lib_dirs: &[PathBuf],
    ) -> Result<impl Stream<Item = Result<CompileEvent>>> {
        let instance = *self.instance();
        // Each `library` entry must be a single-library root dir.
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
                }
                Ok(None) => return None,
                Err(status) => return Some((Err(Error::Grpc(status)), (stream, true))),
            }
        }
    })
}

fn translate(resp: cli::CompileResponse) -> Option<Result<CompileEvent>> {
    use cli::compile_response::Message;

    match resp.message? {
        Message::OutStream(bytes) | Message::ErrStream(bytes) => Some(Ok(CompileEvent::Log(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Message::Progress(progress) => Some(Ok(CompileEvent::Progress {
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

/// The `.ino.<ext>` suffix deliberately skips merged variants like `*.ino.with_bootloader.hex`.
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

/// Offsets mirror the core's `merge-bin` recipe; only the bootloader's varies by chip.
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

fn find_binary(dir: &Path, ext: &str) -> Option<PathBuf> {
    let suffix = format!(".ino.{ext}");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        name.to_string_lossy()
            .ends_with(&suffix)
            .then(|| entry.path())
    })
}
