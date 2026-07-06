//! Per-connection state and the read→dispatch→write loop for one browser
//! socket. The selected port (the helper-side `connect` session concept — the
//! daemon itself is connectionless per-port) and the in-flight request ids live
//! here; the latter back `cancel{id}` for the long-running streams (`compile`).

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

/// How many responses may queue toward the socket before backpressure applies.
const RESPONSE_CHANNEL_CAPACITY: usize = 64;

/// How long streamed-text chunks (`log` / `monitorData`) accumulate before the
/// writer flushes a coalesced frame. Caps a flood of tiny chunks at one frame per
/// window per stream; terminal/progress messages bypass this and flush promptly.
const BATCH_COOLDOWN: Duration = Duration::from_millis(100);

/// Cancellation tokens keyed by in-flight request id. Shared (`Arc<Mutex>`) so a
/// spawned `compile` task can deregister itself while the read loop's `cancel`
/// arm fires a token concurrently. Locks are brief and never held across `.await`.
pub type InFlight = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// An open serial monitor. Lives in the [`Session`] (not a detached task) because
/// it spans three WS requests: `monitorOpen` creates it, `monitorWrite` pushes
/// bytes through `cmd_tx`, and `monitorClose` (or teardown) ends it. `task` is the
/// pump that translates inbound serial into `monitorData` under the open request's
/// id; dropping `cmd_tx` ends the outbound stream, which winds the pump down.
pub struct MonitorSession {
    pub cmd_tx: mpsc::Sender<MonitorCommand>,
    pub task: JoinHandle<()>,
}

/// State for one browser WS connection.
pub struct Session {
    daemon: Arc<Daemon>,
    /// The served pack directory, shared so `compile` (M3+) can resolve a
    /// `{pack, lib}` reference to a local library dir for the daemon.
    resource_root: Arc<ResourceRoot>,
    /// The port chosen via `connect`, if any (opaque to the JS side).
    selected_port: Option<String>,
    /// Cancellation tokens for in-flight long-running requests, for `cancel{id}`.
    in_flight: InFlight,
    /// Scratch space for materialized sketches and their compiled artifacts.
    /// Lazily created on first `compile` and shared into compile tasks via `Arc`,
    /// so it outlives any one task — an artifact must survive until a later
    /// `upload` (M3) reads it, hence session scope rather than request scope.
    temp_base: Option<Arc<TempDir>>,
    /// The open serial monitor, if any. At most one per session (one board).
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

    /// A clone of the daemon handle, for dispatch arms that need a gRPC client.
    pub fn daemon(&self) -> Arc<Daemon> {
        self.daemon.clone()
    }

    /// A clone of the resource-root handle, for `compile` lib resolution.
    pub fn resource_root(&self) -> Arc<ResourceRoot> {
        self.resource_root.clone()
    }

    /// A clone of the in-flight handle, for spawned tasks to register/deregister.
    pub fn in_flight(&self) -> InFlight {
        self.in_flight.clone()
    }

    /// The session scratch dir, creating it on first use. Shared into compile
    /// tasks; the directory is removed once the session and every in-flight
    /// compile have dropped their `Arc`.
    pub fn ensure_temp_base(&mut self) -> Result<Arc<TempDir>> {
        if self.temp_base.is_none() {
            self.temp_base = Some(Arc::new(TempDir::new("thingblock-link")?));
        }
        Ok(self.temp_base.clone().expect("temp_base set above"))
    }

    /// Store the port chosen via `connect` as this session's selected port.
    pub fn select_port(&mut self, port: String) {
        self.selected_port = Some(port);
    }

    /// Clear the selected port (via `disconnect`).
    pub fn clear_port(&mut self) {
        self.selected_port = None;
    }

    /// Whether a serial monitor is currently open.
    pub fn has_monitor(&self) -> bool {
        self.monitor.is_some()
    }

    /// Install the open monitor (via `monitorOpen`). Replaces any prior one — the
    /// bridge rejects a re-open while one is live, so this is only set on a fresh
    /// open.
    pub fn set_monitor(&mut self, monitor: MonitorSession) {
        self.monitor = Some(monitor);
    }

    /// A clone of the open monitor's command sender, for `monitorWrite`. `None` if
    /// no monitor is open.
    pub fn monitor_cmd_tx(&self) -> Option<mpsc::Sender<MonitorCommand>> {
        self.monitor.as_ref().map(|m| m.cmd_tx.clone())
    }

    /// Close the open monitor, if any: ask the daemon to close the port gracefully,
    /// then drop `cmd_tx` so the outbound stream ends and the pump task winds down.
    /// Best-effort — a closed channel or finished task is fine.
    pub async fn close_monitor(&mut self) {
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.cmd_tx.send(MonitorCommand::Close).await;
            drop(monitor.cmd_tx);
            let _ = monitor.task.await;
        }
    }

    /// Drive the connection until the socket closes: a writer task pumps queued
    /// responses to the sink while this loop reads and dispatches requests.
    pub async fn run(mut self, socket: WebSocket) {
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::channel::<Response>(RESPONSE_CHANNEL_CAPACITY);

        let writer = tokio::spawn(async move {
            // Streamed-text chunks accumulate here over a `BATCH_COOLDOWN` window,
            // coalesced per `id`; `flush_at` arms the trailing-edge flush. Other
            // messages flush this buffer first (to keep order) then send promptly.
            let mut buf: Vec<Response> = Vec::new();
            let mut flush_at: Option<Instant> = None;

            loop {
                // Disabled (pends forever) until a chunk arms `flush_at`.
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
                            break; // socket closed
                        }
                        flush_at = None;
                    }
                    message = rx.recv() => match message {
                        // All senders dropped (session teardown): flush and finish.
                        None => {
                            flush(&mut sink, &mut buf).await;
                            break;
                        }
                        Some(response) if is_batchable(&response.body) => {
                            push_coalesced(&mut buf, response);
                            flush_at.get_or_insert_with(|| Instant::now() + BATCH_COOLDOWN);
                        }
                        // Terminal/progress/event: preserve order by flushing the
                        // buffered streamed text first, then send this promptly.
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
                // No binary/ping/pong handling in the protocol; ignore.
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
            }
        }

        // Browser gone: close any open monitor (releasing the port) and cancel any
        // in-flight compiles so detached tasks wind down (and release their
        // `Arc<TempDir>`) rather than running to completion.
        self.close_monitor().await;
        for (_, token) in self.in_flight.lock().expect("in_flight mutex").drain() {
            token.cancel();
        }

        // Dropping `tx` ends the writer task, which closes the sink.
        drop(tx);
        let _ = writer.await;
    }

    /// Parse one text frame as a request envelope and dispatch it.
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

/// Serialize and send one response to the socket. Returns `false` when the socket
/// is closed (the caller should stop writing). A serialization failure is a bug in
/// our own response types — logged and skipped, but the socket stays usable.
async fn send_response(sink: &mut SplitSink<WebSocket, Message>, response: &Response) -> bool {
    match serde_json::to_string(response) {
        Ok(json) => sink.send(Message::Text(json.into())).await.is_ok(),
        Err(e) => {
            warn!(error = %e, "failed to serialize response");
            true
        }
    }
}

/// Drain the coalesced buffer to the socket in order. Returns `false` (stop) if the
/// socket closes mid-flush, leaving any unsent responses dropped with it.
async fn flush(sink: &mut SplitSink<WebSocket, Message>, buf: &mut Vec<Response>) -> bool {
    for response in buf.drain(..) {
        if !send_response(sink, &response).await {
            return false;
        }
    }
    true
}
