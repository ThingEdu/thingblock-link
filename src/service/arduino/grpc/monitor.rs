//! `Monitor` is bidirectional: the first outbound message opens the port, later ones write or close.

use futures::Stream;
use tokio::sync::mpsc;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::service::arduino::grpc::{Client, cli};

#[derive(Debug)]
pub enum MonitorEvent {
    Opened,
    Data(String),
    Error(String),
}

#[derive(Debug)]
pub enum MonitorCommand {
    Write(Vec<u8>),
    Close,
}

impl Client {
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

fn outbound_stream(
    open: cli::MonitorPortOpenRequest,
    cmd_rx: mpsc::Receiver<MonitorCommand>,
) -> impl Stream<Item = cli::MonitorRequest> {
    use cli::monitor_request::Message;

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
            None => None,
        }
    })
}

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

fn translate(resp: cli::MonitorResponse) -> Option<Result<MonitorEvent>> {
    use cli::monitor_response::Message;

    match resp.message? {
        Message::Success(_) => Some(Ok(MonitorEvent::Opened)),
        Message::RxData(bytes) => Some(Ok(MonitorEvent::Data(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Message::Error(message) => Some(Ok(MonitorEvent::Error(message))),
        Message::AppliedSettings(_) => None,
    }
}
