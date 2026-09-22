use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::error::Result;
use crate::server::batch::{is_batchable, push_coalesced};
use crate::server::protocol::{Request, Response, ResponseBody};
use crate::service::arduino::bridge::{self, Responder};
use crate::service::arduino::daemon::Daemon;
use crate::service::arduino::grpc::monitor::MonitorCommand;
use crate::service::resource::ResourceRoot;
use crate::utils::tempdir::TempDir;

const RESPONSE_CHANNEL_CAPACITY: usize = 64;

const BATCH_COOLDOWN: Duration = Duration::from_millis(100);

/// Never hold the lock across `.await`.
pub type InFlight = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// Dropping `cmd_tx` ends the outbound stream, which winds `task` down.
pub struct MonitorSession {
    pub cmd_tx: mpsc::Sender<MonitorCommand>,
    pub task: JoinHandle<()>,
}

pub struct Session {
    daemon: Arc<Daemon>,
    resource_root: Arc<ResourceRoot>,
    selected_port: Option<String>,
    in_flight: InFlight,
    /// Session-scoped: a compiled artifact must survive until a later `upload` reads it.
    temp_base: Option<Arc<TempDir>>,
    monitor: Option<MonitorSession>,
}

impl Session {
    pub fn new(daemon: Arc<Daemon>, resource_root: Arc<ResourceRoot>) -> Self {
        Self {
            daemon,
            resource_root,
            selected_port: None,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            temp_base: None,
            monitor: None,
        }
    }

    pub fn daemon(&self) -> Arc<Daemon> {
        self.daemon.clone()
    }

    pub fn resource_root(&self) -> Arc<ResourceRoot> {
        self.resource_root.clone()
    }

    pub fn in_flight(&self) -> InFlight {
        self.in_flight.clone()
    }

    pub fn ensure_temp_base(&mut self) -> Result<Arc<TempDir>> {
        if self.temp_base.is_none() {
            self.temp_base = Some(Arc::new(TempDir::new("thingblock-link")?));
        }
        Ok(self.temp_base.clone().expect("temp_base set above"))
    }

    pub fn select_port(&mut self, port: String) {
        self.selected_port = Some(port);
    }

    pub fn clear_port(&mut self) {
        self.selected_port = None;
    }

    pub fn has_monitor(&self) -> bool {
        self.monitor.is_some()
    }

    pub fn set_monitor(&mut self, monitor: MonitorSession) {
        self.monitor = Some(monitor);
    }

    pub fn monitor_cmd_tx(&self) -> Option<mpsc::Sender<MonitorCommand>> {
        self.monitor.as_ref().map(|m| m.cmd_tx.clone())
    }

    pub async fn close_monitor(&mut self) {
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.cmd_tx.send(MonitorCommand::Close).await;
            drop(monitor.cmd_tx);
            let _ = monitor.task.await;
        }
    }

    pub async fn run(mut self, socket: WebSocket) {
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::channel::<Response>(RESPONSE_CHANNEL_CAPACITY);

        let writer = tokio::spawn(async move {
            let mut buf: Vec<Response> = Vec::new();
            let mut flush_at: Option<Instant> = None;

            loop {
                let tick = async {
                    match flush_at {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                };

                tokio::select! {
                    biased;
                    () = tick => {
                        if !flush(&mut sink, &mut buf).await {
                            break;
                        }
                        flush_at = None;
                    }
                    message = rx.recv() => match message {
                        None => {
                            flush(&mut sink, &mut buf).await;
                            break;
                        }
                        Some(response) if is_batchable(&response.body) => {
                            push_coalesced(&mut buf, response);
                            flush_at.get_or_insert_with(|| Instant::now() + BATCH_COOLDOWN);
                        }
                        // Flush buffered text first to preserve order.
                        Some(response) => {
                            if !flush(&mut sink, &mut buf).await {
                                break;
                            }
                            flush_at = None;
                            if !send_response(&mut sink, &response).await {
                                break;
                            }
                        }
                    },
                }
            }
        });

        while let Some(message) = stream.next().await {
            let message = match message {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "ws receive error; closing session");
                    break;
                }
            };

            match message {
                Message::Text(text) => self.handle_text(text.as_str(), &tx).await,
                Message::Close(_) => break,
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
            }
        }

        // Release the port, and cancel compiles so they drop their `Arc<TempDir>` instead of finishing.
        self.close_monitor().await;
        for (_, token) in self.in_flight.lock().expect("in_flight mutex").drain() {
            token.cancel();
        }

        drop(tx);
        let _ = writer.await;
    }

    async fn handle_text(&mut self, text: &str, tx: &mpsc::Sender<Response>) {
        let request: Request = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(e) => {
                debug!(error = %e, "rejecting malformed envelope");
                // No id to correlate against on a parse failure.
                let _ = tx
                    .send(Response {
                        id: String::new(),
                        body: ResponseBody::Error {
                            code: "invalidRequest".into(),
                            message: format!("malformed envelope: {e}"),
                        },
                    })
                    .await;
                return;
            }
        };

        let responder = Responder::new(request.id, tx.clone());
        if let Err(e) = bridge::dispatch(self, request.body, &responder).await {
            warn!(id = %responder.id(), code = e.code(), error = %e, "request failed");
            responder
                .send(ResponseBody::Error {
                    code: e.code().into(),
                    message: e.to_string(),
                })
                .await;
        }
    }
}

/// Returns `false` once the socket is closed; a serialization failure is logged and skipped.
async fn send_response(sink: &mut SplitSink<WebSocket, Message>, response: &Response) -> bool {
    match serde_json::to_string(response) {
        Ok(json) => sink.send(Message::Text(json.into())).await.is_ok(),
        Err(e) => {
            warn!(error = %e, "failed to serialize response");
            true
        }
    }
}

async fn flush(sink: &mut SplitSink<WebSocket, Message>, buf: &mut Vec<Response>) -> bool {
    for response in buf.drain(..) {
        if !send_response(sink, &response).await {
            return false;
        }
    }
    true
}
