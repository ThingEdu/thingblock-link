//! The `/ws/arduino` WebSocket session: reads `{id, type, payload}` requests from the editor,
//! runs them against the arduino service, and streams responses back under the request's `id`.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::gateway::GatewayContext;
use crate::gateway::http_local::model_ws::{WsRequest, WsRequestBody, WsResponse, WsResponseBody};
use crate::service::arduino::ArduinoService;
use crate::service::arduino::grpc::compile::CompileEvent;
use crate::service::arduino::grpc::monitor::{MonitorCommand, MonitorEvent};
use crate::service::arduino::grpc::platform::PlatformInstallEvent;
use crate::service::arduino::grpc::upload::UploadEvent;
use crate::utils::tempdir::TempDir;

/// How many responses may queue toward the socket before backpressure applies.
const RESPONSE_CHANNEL_CAPACITY: usize = 64;

/// How long streamed text accumulates before the writer flushes one coalesced frame.
const BATCH_COOLDOWN: Duration = Duration::from_millis(100);

/// How many monitor writes/close may queue toward the daemon.
const MONITOR_COMMAND_CAPACITY: usize = 32;

/// `GET /ws/arduino`: upgrades to WebSocket and runs one [`WsSession`] on the socket.
pub async fn ws(ws: WebSocketUpgrade, State(context): State<GatewayContext>) -> Response {
    ws.on_upgrade(move |socket| WsSession::new(context).run(socket))
}

/// An open serial monitor: `task` pumps serial data out, `cmd_tx` carries writes/close in.
struct Monitor {
    cmd_tx: mpsc::Sender<MonitorCommand>,
    task: JoinHandle<()>,
}

/// State of one editor WS connection.
struct WsSession {
    context: GatewayContext,
    /// The port bound by `connect`, used by upload, flash and monitor.
    port: Option<String>,
    /// Cancellation tokens of spawned requests by id, shared with the tasks that deregister.
    in_flight: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Holds materialized sketches, created on the first `compile`.
    temp_base: Option<TempDir>,
    monitor: Option<Monitor>,
}

/// Sends one request's responses to the writer, stamped with its `id`.
#[derive(Clone)]
struct Responder {
    id: String,
    tx: mpsc::Sender<WsResponse>,
}

impl Responder {
    /// A closed channel means the browser is gone, so the response is logged and dropped.
    async fn send(&self, body: WsResponseBody) {
        let response = WsResponse {
            id: self.id.clone(),
            body,
        };
        if self.tx.send(response).await.is_err() {
            warn!(id = %self.id, "ws response dropped: writer closed");
        }
    }

    /// Sends a terminal `error` with one of the three WS codes.
    async fn send_error(&self, error: &Error) {
        let code = match error {
            Error::Cancelled => "cancelled",
            Error::Gateway(_) => "invalidRequest",
            _ => "failed",
        };
        if matches!(error, Error::Cancelled) {
            debug!(id = %self.id, "ws request cancelled");
        } else {
            warn!(id = %self.id, code, %error, "ws request failed");
        }
        self.send(WsResponseBody::Error {
            code: code.into(),
            message: error.to_string(),
        })
        .await;
    }
}

impl WsSession {
    fn new(context: GatewayContext) -> Self {
        Self {
            context,
            port: None,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            temp_base: None,
            monitor: None,
        }
    }

    /// Reads and dispatches requests until the socket closes, then tears the session down.
    async fn run(mut self, socket: WebSocket) {
        let (sink, mut stream) = socket.split();
        let (tx, rx) = mpsc::channel(RESPONSE_CHANNEL_CAPACITY);
        let writer = tokio::spawn(write_loop(sink, rx));

        while let Some(message) = stream.next().await {
            match message {
                Ok(Message::Text(text)) => self.handle_text(text.as_str(), &tx).await,
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "ws receive error; closing session");
                    break;
                }
            }
        }

        self.disconnect().await;
        // The writer ends once this and every spawned task's sender are dropped.
        drop(tx);
        let _ = writer.await;
    }

    /// Parses one frame and dispatches it; a failed dispatch is the request's terminal `error`.
    async fn handle_text(&mut self, text: &str, tx: &mpsc::Sender<WsResponse>) {
        let request: WsRequest = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(e) => {
                // A malformed envelope has no id to correlate the error with.
                let responder = Responder {
                    id: String::new(),
                    tx: tx.clone(),
                };
                responder
                    .send_error(&Error::Gateway(format!("malformed envelope: {e}")))
                    .await;
                return;
            }
        };

        let responder = Responder {
            id: request.id,
            tx: tx.clone(),
        };
        if let Err(e) = self.dispatch(request.body, &responder).await {
            responder.send_error(&e).await;
        }
    }

    /// Runs one request. Each request ends with exactly one terminal response: either this
    /// returns `Err`, or it (or the task it spawns) sends the `result`/`error`. `monitorWrite`
    /// and `cancel` have no reply of their own.
    async fn dispatch(&mut self, body: WsRequestBody, responder: &Responder) -> Result<()> {
        let arduino = self.context.arduino.clone();

        match body {
            WsRequestBody::Connect { port, pnpid } => {
                if let Some(connected) = &self.port {
                    return Err(Error::Gateway(format!("already connected to {connected}")));
                }
                let targets = arduino.client().board_list(&pnpid).await?;
                if !targets.iter().any(|target| target.port == port) {
                    return Err(Error::Gateway(format!("no matching board on {port}")));
                }
                debug!(id = responder.id, %port, "ws connect");
                self.port = Some(port);
                responder.send(WsResponseBody::Ok {}).await;
            }
            WsRequestBody::Disconnect {} => {
                self.disconnect().await;
                debug!(id = responder.id, "ws disconnect");
                responder.send(WsResponseBody::Ok {}).await;
            }
            WsRequestBody::Compile {
                fqbn,
                options,
                source,
                libs,
            } => {
                let resource_root = self.context.thingblock.resource_root();
                let lib_dirs = libs
                    .iter()
                    .map(|lib| resource_root.resolve_lib_dir(&lib.pack, &lib.lib))
                    .collect::<Result<Vec<_>>>()?;
                let sketch_dir = write_sketch(self.temp_base()?, &responder.id, &source)?;

                let task_responder = responder.clone();
                self.spawn(responder, async move {
                    let mut client = arduino.client();
                    let stream = client
                        .compile(&fqbn, &sketch_dir, &options, &lib_dirs)
                        .await?;
                    let mut stream = pin!(stream);
                    while let Some(event) = stream.next().await {
                        match event? {
                            CompileEvent::Log(chunk) => {
                                task_responder.send(WsResponseBody::Log { chunk }).await;
                            }
                            CompileEvent::Progress { phase, percent } => {
                                task_responder
                                    .send(WsResponseBody::Progress { phase, percent })
                                    .await;
                            }
                            CompileEvent::Done(artifact) => {
                                return Ok(WsResponseBody::Compile { artifact });
                            }
                        }
                    }
                    Err(Error::Arduino(
                        "compile ended without producing an artifact".into(),
                    ))
                });
            }
            WsRequestBody::Upload {
                fqbn,
                upload_speed,
                artifact,
            } => {
                let port = self.connected_port()?;
                let task_responder = responder.clone();
                self.spawn(responder, async move {
                    upload(
                        &arduino,
                        &task_responder,
                        &fqbn,
                        &artifact.path,
                        &port,
                        upload_speed,
                    )
                    .await
                });
            }
            WsRequestBody::FlashFirmware {
                fqbn,
                upload_speed,
                pack,
                file,
            } => {
                let port = self.connected_port()?;
                let image = self
                    .context
                    .thingblock
                    .resource_root()
                    .resolve_firmware_file(&pack, &file)?;
                let staged = stage_firmware_image(&image)?;
                let staged_image = staged
                    .path()
                    .join(image.file_name().expect("firmware image is a file"));

                let task_responder = responder.clone();
                self.spawn(responder, async move {
                    // Held until the flash ends: dropping it deletes the staged image mid-flash.
                    let _staged = staged;
                    upload(
                        &arduino,
                        &task_responder,
                        &fqbn,
                        &staged_image.to_string_lossy(),
                        &port,
                        upload_speed,
                    )
                    .await
                });
            }
            WsRequestBody::MonitorOpen { baud_rate } => {
                let port = self.connected_port()?;
                self.open_monitor(responder, &port, baud_rate).await?;
            }
            WsRequestBody::MonitorWrite { data } => {
                let Some(monitor) = &self.monitor else {
                    return Err(Error::Gateway("monitorWrite with no monitor open".into()));
                };
                if monitor
                    .cmd_tx
                    .send(MonitorCommand::Write(data.into_bytes()))
                    .await
                    .is_err()
                {
                    warn!(
                        id = responder.id,
                        "monitorWrite dropped: monitor stream closed"
                    );
                }
            }
            WsRequestBody::MonitorClose {} => {
                self.close_monitor().await;
                responder.send(WsResponseBody::Ok {}).await;
            }
            WsRequestBody::InstallPlatform { platform } => {
                // Installs mutate the shared data dir and end with a reinit, so they can't overlap.
                let Ok(install_guard) = arduino.install_lock().try_lock_owned() else {
                    return Err(Error::Gateway(
                        "a platform install is already running".into(),
                    ));
                };

                let task_responder = responder.clone();
                self.spawn(responder, async move {
                    let _install_guard = install_guard;
                    let mut client = arduino.client();
                    let stream = client.platform_install(platform).await?;
                    let mut stream = pin!(stream);
                    while let Some(event) = stream.next().await {
                        match event? {
                            PlatformInstallEvent::Log(chunk) => {
                                task_responder.send(WsResponseBody::Log { chunk }).await;
                            }
                            PlatformInstallEvent::Progress { phase, percent } => {
                                task_responder
                                    .send(WsResponseBody::Progress { phase, percent })
                                    .await;
                            }
                            PlatformInstallEvent::Done => {
                                // The new core stays invisible to compiles until `Init` re-runs.
                                arduino.reinit().await?;
                                return Ok(WsResponseBody::Ok {});
                            }
                        }
                    }
                    Err(Error::Arduino(
                        "platform install ended without completing".into(),
                    ))
                });
            }
            // The envelope `id` is the in-flight request's id; its task sends `error{cancelled}`.
            WsRequestBody::Cancel {} => {
                match self
                    .in_flight
                    .lock()
                    .expect("in_flight mutex")
                    .get(&responder.id)
                {
                    Some(token) => token.cancel(),
                    None => debug!(id = responder.id, "cancel: no in-flight request"),
                }
            }
        }

        Ok(())
    }

    /// Runs `task` in the background as a cancellable request; its output is the terminal reply.
    /// Cancelling drops `task`, which drops its gRPC stream and so ends the daemon call.
    fn spawn<F>(&self, responder: &Responder, task: F)
    where
        F: Future<Output = Result<WsResponseBody>> + Send + 'static,
    {
        let token = CancellationToken::new();
        self.in_flight
            .lock()
            .expect("in_flight mutex")
            .insert(responder.id.clone(), token.clone());

        let in_flight = self.in_flight.clone();
        let responder = responder.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                () = token.cancelled() => Err(Error::Cancelled),
                result = task => result,
            };
            match result {
                Ok(body) => responder.send(body).await,
                Err(e) => responder.send_error(&e).await,
            }
            in_flight
                .lock()
                .expect("in_flight mutex")
                .remove(&responder.id);
        });
    }

    /// Opens the monitor, replies `result {}` once the port is confirmed open, then spawns the
    /// pump that streams `monitorData` under this request's id.
    async fn open_monitor(
        &mut self,
        responder: &Responder,
        port: &str,
        baud_rate: u32,
    ) -> Result<()> {
        // One monitor per session: an open (or port-errored) one is closed before reopening.
        self.close_monitor().await;

        let (cmd_tx, cmd_rx) = mpsc::channel(MONITOR_COMMAND_CAPACITY);
        let mut stream = Box::pin(
            self.context
                .arduino
                .client()
                .monitor(port, baud_rate, cmd_rx)
                .await?,
        );

        match stream.next().await {
            Some(Ok(MonitorEvent::Opened)) => {}
            Some(Ok(MonitorEvent::Error(message))) => return Err(Error::Arduino(message)),
            Some(Ok(MonitorEvent::Data(_))) => {
                return Err(Error::Arduino(
                    "monitor streamed data before confirming the port opened".into(),
                ));
            }
            Some(Err(e)) => return Err(e),
            None => {
                return Err(Error::Arduino(
                    "monitor stream closed before opening".into(),
                ));
            }
        }
        responder.send(WsResponseBody::Ok {}).await;

        let pump_responder = responder.clone();
        let task = tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                match event {
                    Ok(MonitorEvent::Data(data)) => {
                        pump_responder
                            .send(WsResponseBody::MonitorData { data })
                            .await;
                    }
                    Ok(MonitorEvent::Opened) => {}
                    Ok(MonitorEvent::Error(message)) => {
                        return pump_responder.send_error(&Error::Arduino(message)).await;
                    }
                    Err(e) => return pump_responder.send_error(&e).await,
                }
            }
        });
        self.monitor = Some(Monitor { cmd_tx, task });
        Ok(())
    }

    /// Asks the daemon to release the port, then waits for the pump to end. A no-op without a
    /// monitor.
    async fn close_monitor(&mut self) {
        if let Some(Monitor { cmd_tx, task }) = self.monitor.take() {
            let _ = cmd_tx.send(MonitorCommand::Close).await;
            // Dropping the sender ends the outbound stream, so the daemon closes its side.
            drop(cmd_tx);
            let _ = task.await;
        }
    }

    /// Closes the monitor, cancels every in-flight request and clears the connected port.
    async fn disconnect(&mut self) {
        self.close_monitor().await;
        for (_, token) in self.in_flight.lock().expect("in_flight mutex").drain() {
            token.cancel();
        }
        self.port = None;
    }

    fn connected_port(&self) -> Result<String> {
        self.port
            .clone()
            .ok_or_else(|| Error::Gateway("not connected; send connect first".into()))
    }

    /// The sketch scratch dir, created on first use and removed with the session.
    fn temp_base(&mut self) -> Result<&Path> {
        if self.temp_base.is_none() {
            let dir = TempDir::new("thingblock-link")
                .map_err(|e| Error::Arduino(format!("create sketch temp dir: {e}")))?;
            self.temp_base = Some(dir);
        }
        Ok(self.temp_base.as_ref().expect("temp_base set above").path())
    }
}

/// Uploads `image` to `port`, streaming `log`s; returns the terminal `result {}`.
async fn upload(
    arduino: &ArduinoService,
    responder: &Responder,
    fqbn: &str,
    image: &str,
    port: &str,
    upload_speed: u32,
) -> Result<WsResponseBody> {
    let mut client = arduino.client();
    let stream = client.upload(fqbn, image, port, upload_speed).await?;
    let mut stream = pin!(stream);
    while let Some(event) = stream.next().await {
        match event? {
            UploadEvent::Log(chunk) => responder.send(WsResponseBody::Log { chunk }).await,
            UploadEvent::Done => return Ok(WsResponseBody::Ok {}),
        }
    }
    Err(Error::Arduino("upload ended without completing".into()))
}

/// Pumps queued responses to the socket. Streamed text is held for [`BATCH_COOLDOWN`] and merged
/// per id; any other response flushes the held text first so order is kept.
async fn write_loop(mut sink: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<WsResponse>) {
    let mut buf: Vec<WsResponse> = Vec::new();
    let mut flush_at: Option<Instant> = None;

    loop {
        let tick = async {
            match flush_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            biased;
            () = tick => {
                if !flush(&mut sink, &mut buf).await {
                    return;
                }
                flush_at = None;
            }
            response = rx.recv() => match response {
                None => {
                    flush(&mut sink, &mut buf).await;
                    return;
                }
                Some(response) if matches!(
                    response.body,
                    WsResponseBody::Log { .. } | WsResponseBody::MonitorData { .. }
                ) => {
                    push_coalesced(&mut buf, response);
                    flush_at.get_or_insert_with(|| Instant::now() + BATCH_COOLDOWN);
                }
                Some(response) => {
                    buf.push(response);
                    if !flush(&mut sink, &mut buf).await {
                        return;
                    }
                    flush_at = None;
                }
            },
        }
    }
}

/// Appends `response`, merging it into the last entry when both are the same id and text kind.
fn push_coalesced(buf: &mut Vec<WsResponse>, response: WsResponse) {
    if let Some(last) = buf.last_mut()
        && last.id == response.id
    {
        match (&mut last.body, &response.body) {
            (WsResponseBody::Log { chunk }, WsResponseBody::Log { chunk: more }) => {
                chunk.push_str(more);
                return;
            }
            (WsResponseBody::MonitorData { data }, WsResponseBody::MonitorData { data: more }) => {
                data.push_str(more);
                return;
            }
            _ => {}
        }
    }
    buf.push(response);
}

/// Sends the buffered responses in order; `false` once the socket is closed.
async fn flush(sink: &mut SplitSink<WebSocket, Message>, buf: &mut Vec<WsResponse>) -> bool {
    for response in buf.drain(..) {
        let json = serde_json::to_string(&response).expect("WsResponse always serializes");
        if sink.send(Message::Text(json.into())).await.is_err() {
            return false;
        }
    }
    true
}

/// Writes `source` as `<base>/sketch_<id>/sketch_<id>.ino`: arduino-cli requires the main `.ino`
/// to share its folder's name, and the prefix keeps empty or digit-leading ids valid.
fn write_sketch(base: &Path, id: &str, source: &str) -> Result<PathBuf> {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = format!("sketch_{safe}");
    let dir = base.join(&name);
    let io = |e: std::io::Error| Error::Arduino(format!("write sketch {}: {e}", dir.display()));
    std::fs::create_dir_all(&dir).map_err(io)?;
    std::fs::write(dir.join(format!("{name}.ino")), source).map_err(io)?;
    Ok(dir)
}

/// Copies the image's directory to a temp dir: esp32 needs the bootloader/partitions beside the
/// image, and esptool writes `*_flashed.bin` next to its inputs, which must not land in the pack.
fn stage_firmware_image(image: &Path) -> Result<TempDir> {
    let io =
        |e: std::io::Error| Error::Thingblock(format!("stage firmware {}: {e}", image.display()));
    let src_dir = image.parent().expect("firmware image has a parent dir");
    let staged = TempDir::new("thingblock-link-firmware").map_err(io)?;
    for entry in std::fs::read_dir(src_dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        if entry.file_type().map_err(io)?.is_file() {
            std::fs::copy(entry.path(), staged.path().join(entry.file_name())).map_err(io)?;
        }
    }
    Ok(staged)
}
