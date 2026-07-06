//! `Monitor` translation: drive arduino-cli's bidirectional `Monitor` stream and
//! map it onto helper-shaped [`MonitorEvent`]s. Backs the WS `monitorOpen` /
//! `monitorWrite` / `monitorClose` request family.
//!
//! Unlike `Compile`/`Upload` (one request, server-stream to a terminal), `Monitor`
//! is bidirectional and long-lived: the first outbound message opens the port, and
//! later outbound messages carry serial bytes to write or a graceful close. The
//! session feeds those later messages in through a [`MonitorCommand`] channel; this
//! module turns the leading open request plus that channel into the `MonitorRequest`
//! stream tonic wants, and translates inbound `MonitorResponse`s back.
//!
//! The arduino-cli schema never leaks past this module — the bridge sees only
//! [`MonitorEvent`] and [`MonitorCommand`].

use futures::Stream;
use tokio::sync::mpsc;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::service::arduino::grpc::{Client, cli};

/// One translated step of a monitor session, in the helper's own shapes.
#[derive(Debug)]
pub enum MonitorEvent {
    /// The port opened successfully — the first event of a healthy session.
    Opened,
    /// A chunk of serial bytes received from the port.
    Data(String),
    /// A port-level error reported by the daemon (e.g. the board vanished).
    Error(String),
}

/// A helper-side command pushed into an open monitor's outbound stream. Keeps the
/// `cli::MonitorRequest` shape out of the session/bridge; this module maps each to
/// the matching `MonitorRequest`.
#[derive(Debug)]
pub enum MonitorCommand {
    /// Serial bytes to transmit to the port.
    Write(Vec<u8>),
    /// Gracefully close the port; the daemon ends the stream after the close lands.
    Close,
}

impl Client {
    /// Open `port` at `baud_rate` and stream translated events. `cmd_rx` feeds the
    /// outbound side after the open: each [`MonitorCommand`] becomes a `tx_data` or
    /// `close` message. The returned stream yields `Opened` first (on success), then
    /// `Data` chunks, and ends after a `close`, an `Error`, or the channel closing.
    pub async fn monitor(
        &mut self,
        port: &str,
        baud_rate: u32,
        cmd_rx: mpsc::Receiver<MonitorCommand>,
        // The returned stream is fully owned (it holds the tonic `Streaming` and the
        // command channel), so it captures none of this call's lifetimes — letting
        // the bridge move it into a detached pump task (`+ use<>`).
    ) -> Result<impl Stream<Item = Result<MonitorEvent>> + use<>> {
        let open = build_open_request(*self.instance(), port, baud_rate);
        let outbound = outbound_stream(open, cmd_rx);
        let inbound = self.inner().monitor(outbound).await?.into_inner();
        Ok(into_events(inbound))
    }
}

/// Assemble the `MonitorPortOpenRequest`. Pure (no I/O, no daemon) so the
/// WS-payload → gRPC mapping is unit-testable without hardware.
///
/// The WS payload carries only a port *address* and a baud rate; for the
/// local-helper USB-board case the protocol is `serial`, and the baud rate is a
/// `baudrate` port setting. `fqbn` is left empty — it only disambiguates when more
/// than one platform provides a monitor for the protocol, which serial does not.
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

/// Build the outbound `MonitorRequest` stream: the open request first, then one
/// message per [`MonitorCommand`] until the channel closes. A `Close` command emits
/// the close message and ends the stream so the daemon shuts the port down.
fn outbound_stream(
    open: cli::MonitorPortOpenRequest,
    cmd_rx: mpsc::Receiver<MonitorCommand>,
) -> impl Stream<Item = cli::MonitorRequest> {
    use cli::monitor_request::Message;

    // State: `Some(open)` until the leading open request is yielded, then drive the
    // command channel; `done` short-circuits after a `Close`.
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

/// Adapt the tonic `Monitor` stream into a `MonitorEvent` stream, skipping empty
/// frames and terminating after the first error.
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
                    // Empty oneof — nothing to surface; keep reading.
                }
                Ok(None) => return None, // stream ended cleanly
                Err(status) => return Some((Err(Error::Grpc(status)), (stream, true))),
            }
        }
    })
}

/// Map one `MonitorResponse` to a `MonitorEvent`, or `None` for an empty frame.
/// Serial bytes are decoded UTF-8-lossily, matching how compile/upload log chunks
/// are surfaced (the editor's `SerialLog` consumes text).
fn translate(resp: cli::MonitorResponse) -> Option<Result<MonitorEvent>> {
    use cli::monitor_response::Message;

    match resp.message? {
        Message::Success(_) => Some(Ok(MonitorEvent::Opened)),
        Message::RxData(bytes) => Some(Ok(MonitorEvent::Data(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Message::Error(message) => Some(Ok(MonitorEvent::Error(message))),
        // `applied_settings` (the port's effective config) is not surfaced.
        Message::AppliedSettings(_) => None,
    }
}
