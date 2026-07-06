//! `Upload` translation: drive arduino-cli's `Upload` server-stream and map its
//! `UploadResponse` oneof onto helper-shaped [`UploadEvent`]s. Backs the WS
//! `upload` request.
//!
//! Two notes on the proto shape, both reflected here:
//! - `UploadResponse` has no structured progress (unlike `CompileResponse`); the
//!   upload tool's progress (avrdude/esptool `####`) arrives inside the log
//!   stream as text. So an upload emits only `Log` chunks and a terminal `Done`.
//! - `UploadRequest` has no raw-binary field; `import_file` is purpose-built to
//!   flash a prebuilt binary and overrides `sketch_path`/`import_dir`. We point it
//!   at the artifact compile produced.
//!
//! The arduino-cli schema never leaks past this module — the bridge sees only
//! [`UploadEvent`].

use futures::Stream;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::service::arduino::grpc::{Client, cli};

/// One translated step of an upload, in the helper's own shapes.
#[derive(Debug)]
pub enum UploadEvent {
    /// A stdout/stderr chunk from the upload tool.
    Log(String),
    /// Terminal success: the flash completed.
    Done,
}

impl Client {
    /// Flash `import_file` to `port` for `fqbn`, streaming translated events. The
    /// stream ends after a `Done` (success) or yields a single `Err` (gRPC status)
    /// and then ends.
    ///
    /// `upload_speed` of `0` defers to the FQBN's `boards.txt`; a non-zero value
    /// overrides it via an `upload.speed` build property.
    pub async fn upload(
        &mut self,
        fqbn: &str,
        import_file: &str,
        port: &str,
        upload_speed: u32,
    ) -> Result<impl Stream<Item = Result<UploadEvent>>> {
        let request = build_request(*self.instance(), fqbn, import_file, port, upload_speed);
        let stream = self.inner().upload(request).await?.into_inner();
        Ok(into_events(stream))
    }
}

/// Assemble the `UploadRequest`. Pure (no I/O, no daemon) so the WS-payload → gRPC
/// mapping is unit-testable without hardware.
///
/// The WS payload carries only a port *address*; arduino-cli needs a protocol to
/// pick the upload tool, which for the local-helper USB-board case is `serial`.
pub fn build_request(
    instance: cli::Instance,
    fqbn: &str,
    import_file: &str,
    port: &str,
    upload_speed: u32,
) -> cli::UploadRequest {
    cli::UploadRequest {
        instance: Some(instance),
        fqbn: fqbn.to_string(),
        import_file: import_file.to_string(),
        port: Some(cli::Port {
            address: port.to_string(),
            protocol: "serial".to_string(),
            ..Default::default()
        }),
        // 0 means "let the FQBN decide"; only override when the editor asked.
        upload_properties: if upload_speed > 0 {
            vec![format!("upload.speed={upload_speed}")]
        } else {
            Vec::new()
        },
        ..Default::default()
    }
}

/// Adapt the tonic `Upload` stream into an `UploadEvent` stream, skipping empty
/// frames and terminating after the first error.
fn into_events(stream: Streaming<cli::UploadResponse>) -> impl Stream<Item = Result<UploadEvent>> {
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

/// Map one `UploadResponse` to an `UploadEvent`, or `None` for an empty frame.
fn translate(resp: cli::UploadResponse) -> Option<Result<UploadEvent>> {
    use cli::upload_response::Message;

    match resp.message? {
        Message::OutStream(bytes) | Message::ErrStream(bytes) => Some(Ok(UploadEvent::Log(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        // `updated_upload_port` (the board's reconnect port) is unused for now;
        // it matters to the monitor (M4), not to flashing.
        Message::Result(_) => Some(Ok(UploadEvent::Done)),
    }
}
