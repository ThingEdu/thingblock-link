//! `Upload` translation backing WS `upload`. `UploadResponse` has no structured progress: the
//! tool's `####` progress arrives as log text, so an upload yields only `Log`s and a `Done`.

use futures::Stream;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::service::arduino::grpc::{Client, cli};

/// One translated step of an upload, in the helper's own shapes.
#[derive(Debug)]
pub enum UploadEvent {
    Log(String),
    /// Terminal success: the flash completed.
    Done,
}

impl Client {
    /// Flashes the prebuilt `import_file` to `port` for `fqbn`, streaming translated events.
    /// `upload_speed` of `0` defers to the FQBN's `boards.txt`; non-zero overrides it.
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

/// Builds the `UploadRequest`; `import_file` flashes a prebuilt binary, overriding `sketch_path`.
/// The WS payload carries only a port address, so the protocol is `serial` for local USB boards.
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
        upload_properties: if upload_speed > 0 {
            vec![format!("upload.speed={upload_speed}")]
        } else {
            Vec::new()
        },
        ..Default::default()
    }
}

/// Adapts the tonic `Upload` stream into `UploadEvent`s, skipping empty frames
/// and ending after the first error.
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
                }
                Ok(None) => return None,
                Err(status) => return Some((Err(Error::Grpc(status)), (stream, true))),
            }
        }
    })
}

/// Maps one `UploadResponse` to an `UploadEvent`, or `None` for an empty frame.
fn translate(resp: cli::UploadResponse) -> Option<Result<UploadEvent>> {
    use cli::upload_response::Message;

    match resp.message? {
        Message::OutStream(bytes) | Message::ErrStream(bytes) => Some(Ok(UploadEvent::Log(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        // `updated_upload_port` (the board's reconnect port) isn't needed for flashing.
        Message::Result(_) => Some(Ok(UploadEvent::Done)),
    }
}
