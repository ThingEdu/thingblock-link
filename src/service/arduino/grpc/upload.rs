//! `UploadResponse` has no structured progress; the tool's `####` progress arrives as log text.

use futures::Stream;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::service::arduino::grpc::{Client, cli};

#[derive(Debug)]
pub enum UploadEvent {
    Log(String),
    Done,
}

impl Client {
    /// `upload_speed` of `0` defers to the FQBN's `boards.txt`.
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

/// The WS payload carries only a port address; `serial` is the protocol for local USB boards.
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

fn translate(resp: cli::UploadResponse) -> Option<Result<UploadEvent>> {
    use cli::upload_response::Message;

    match resp.message? {
        Message::OutStream(bytes) | Message::ErrStream(bytes) => Some(Ok(UploadEvent::Log(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Message::Result(_) => Some(Ok(UploadEvent::Done)),
    }
}
