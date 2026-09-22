//! `Monitor` translation backing WS `monitorOpen`/`monitorWrite`/`monitorClose`. The stream is
//! bidirectional and long-lived: the first outbound message opens the port, later ones write/close.

use futures::Stream;
use tokio::sync::mpsc;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::service::arduino::grpc::{Client, cli};

/// One translated step of a monitor session, in the helper's own shapes.
#[derive(Debug)]
pub enum MonitorEvent {
    /// The port opened; the first event of a healthy session.
    Opened,
    Data(String),
    /// A port-level error from the daemon, e.g. the board vanished.
    Error(String),
}

/// A command pushed into an open monitor's outbound stream, keeping `cli::MonitorRequest`
/// out of the session and bridge.
#[derive(Debug)]
pub enum MonitorCommand {
    Write(Vec<u8>),
    /// Graceful close; the daemon ends the stream once it lands.
    Close,
}

impl Client {
    /// Opens `port` at `baud_rate` and streams `Opened`, then `Data`, until close or error.
    /// `cmd_rx` feeds later writes/close into the outbound side.
    pub async fn monitor(
        &mut self,
        port: &str,
        baud_rate: u32,
        cmd_rx: mpsc::Receiver<MonitorCommand>,
        // `+ use<>`: the stream owns everything, so the bridge can move it into a detached task.
    ) -> Result<impl Stream<Item = Result<MonitorEvent>> + use<>> {
        let open = build_open_request(*self.instance(), port, baud_rate);
        let outbound = outbound_stream(open, cmd_rx);
        let inbound = self.inner().monitor(outbound).await?.into_inner();
        Ok(into_events(inbound))
    }
}

/// Builds the serial `MonitorPortOpenRequest` with a `baudrate` setting; pure so it's testable.
/// `fqbn` stays empty: it only disambiguates multiple monitors for a protocol, never so for serial.
pub fn build_open_request(
    instance: cli::Instance,
    port: &str,
    baud_rate: u32,
) -> cli::MonitorPortOpenRequest {
    cli::MonitorPortOpenRequest {
        instance: Some(instance),
        port: Some(cli::Port {
            address: port.to_string(),
            protocol: "serial".to_string(),
            ..Default::default()
        }),
        port_configuration: Some(cli::MonitorPortConfiguration {
            settings: vec![cli::MonitorPortSetting {
                setting_id: "baudrate".to_string(),
                value: baud_rate.to_string(),
            }],
        }),
        ..Default::default()
    }
}

/// Builds the outbound request stream: the open request, then one message per command.
/// A `Close` sends the close message and ends the stream so the daemon shuts the port.
fn outbound_stream(
    open: cli::MonitorPortOpenRequest,
    cmd_rx: mpsc::Receiver<MonitorCommand>,
) -> impl Stream<Item = cli::MonitorRequest> {
    use cli::monitor_request::Message;

    // `Some(open)` until the open request is yielded; `done` short-circuits after a `Close`.
    let init = (Some(open), cmd_rx, false);
    futures::stream::unfold(init, |(open, mut cmd_rx, done)| async move {
        if done {
            return None;
        }
        if let Some(open) = open {
            let req = cli::MonitorRequest {
                message: Some(Message::OpenRequest(open)),
            };
            return Some((req, (None, cmd_rx, false)));
        }
        match cmd_rx.recv().await {
            Some(MonitorCommand::Write(bytes)) => {
                let req = cli::MonitorRequest {
                    message: Some(Message::TxData(bytes)),
                };
                Some((req, (None, cmd_rx, false)))
            }
            Some(MonitorCommand::Close) => {
                let req = cli::MonitorRequest {
                    message: Some(Message::Close(true)),
                };
                Some((req, (None, cmd_rx, true)))
            }
            // Sender dropped (session torn down): stop without a graceful close.
            None => None,
        }
    })
}

/// Adapts the tonic `Monitor` stream into `MonitorEvent`s, skipping empty frames
/// and ending after the first error.
fn into_events(
    stream: Streaming<cli::MonitorResponse>,
) -> impl Stream<Item = Result<MonitorEvent>> {
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

/// Maps one `MonitorResponse` to a `MonitorEvent`, or `None` for an empty frame.
/// Serial bytes decode UTF-8-lossily because the editor's `SerialLog` consumes text.
fn translate(resp: cli::MonitorResponse) -> Option<Result<MonitorEvent>> {
    use cli::monitor_response::Message;

    match resp.message? {
        Message::Success(_) => Some(Ok(MonitorEvent::Opened)),
        Message::RxData(bytes) => Some(Ok(MonitorEvent::Data(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Message::Error(message) => Some(Ok(MonitorEvent::Error(message))),
        // The port's effective config is not surfaced.
        Message::AppliedSettings(_) => None,
    }
}
