//! Translates the WS envelope into arduino-cli gRPC streaming calls and pumps
//! results back as `log` / `progress` / `result` / `error` (and `monitorData`).
//!
//! This is the only place the two schemas meet; neither leaks past it. A request
//! produces zero or more streamed responses and one terminal `result`/`error`,
//! all sharing the request `id` carried by the [`Responder`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::server::protocol::{
    Artifact, CompileOptions, CompileResult, ListBoardsResult, RequestBody, Response, ResponseBody,
};
use crate::server::session::{InFlight, MonitorSession, Session};
use crate::service::arduino::daemon::Daemon;
use crate::service::arduino::grpc::compile::CompileEvent;
use crate::service::arduino::grpc::monitor::{MonitorCommand, MonitorEvent};
use crate::service::arduino::grpc::platform::PlatformInstallEvent;
use crate::service::arduino::grpc::upload::UploadEvent;
use crate::utils::tempdir::TempDir;

/// How many outbound monitor commands (writes / close) may queue before
/// backpressure applies. Serial writes from the editor are small and infrequent.
const MONITOR_COMMAND_CAPACITY: usize = 32;

/// Sends responses for one request back to the session's writer task, stamping
/// each with the request `id`. Cloneable/`&`-shareable so a streaming handler can
/// emit many `log`/`progress` messages before its terminal reply.
#[derive(Clone)]
pub struct Responder {
    id: String,
    tx: mpsc::Sender<Response>,
}

impl Responder {
    pub fn new(id: String, tx: mpsc::Sender<Response>) -> Self {
        Self { id, tx }
    }

    /// The request `id` this responder stamps onto every reply, for log context.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Send one response body for this request. A closed channel (browser gone)
    /// is not actionable here, so it is logged and dropped.
    pub async fn send(&self, body: ResponseBody) {
        let response = Response {
            id: self.id.clone(),
            body,
        };
        if self.tx.send(response).await.is_err() {
            warn!(id = %self.id, "response dropped: ws writer closed");
        }
    }

    /// Send a terminal `error` derived from an [`Error`].
    async fn send_error(&self, error: &Error) {
        self.send(ResponseBody::Error {
            code: error.code().into(),
            message: error.to_string(),
        })
        .await;
    }
}

/// Dispatch one request body to its gRPC translation, streaming responses back
/// through `responder`.
///
/// Returns `Err` only for failures the session should turn into a terminal
/// `error`; handlers that own their own terminal reply (e.g. the spawned
/// `compile` task) return `Ok`.
pub async fn dispatch(
    session: &mut Session,
    body: RequestBody,
    responder: &Responder,
) -> Result<()> {
    let id = responder.id();

    match body {
        RequestBody::ListBoards { pnpid } => {
            let targets = session.daemon().client().board_list(&pnpid).await?;
            debug!(id, count = targets.len(), "listBoards: returning targets");
            responder
                .send(ResponseBody::Result(
                    serde_json::to_value(ListBoardsResult { targets })
                        .expect("ListBoardsResult always serializes"),
                ))
                .await;
        }
        // The daemon is connectionless per-port, so `connect` is a session-side
        // concept: store the chosen port; existence is checked by upload/monitor.
        RequestBody::Connect { port } => {
            if port.is_empty() {
                return Err(Error::InvalidRequest(
                    "connect requires a non-empty port".into(),
                ));
            }
            debug!(id, %port, "connect: selected port");
            session.select_port(port);
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        RequestBody::Disconnect {} => {
            // Close any open monitor (releasing the port) before clearing it.
            session.close_monitor().await;
            debug!(id, "disconnect: cleared selected port");
            session.clear_port();
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        // Long-running and cancellable: run on a spawned task so the read loop
        // stays responsive (notably to `cancel`). The task owns its terminal.
        RequestBody::Compile {
            fqbn,
            options,
            source,
            libs,
        } => {
            let opts: CompileOptions = serde_json::from_value(options)
                .map_err(|e| Error::InvalidRequest(format!("compile options: {e}")))?;
            // Resolve vendored lib references against the served root before
            // committing the request: a bad ref is a boundary error, so fail fast
            // (terminal `error{resource}`) rather than register a doomed task.
            let resource_root = session.resource_root();
            let lib_dirs = libs
                .iter()
                .map(|r| resource_root.resolve_lib_dir(&r.pack, &r.lib))
                .collect::<Result<Vec<_>>>()?;
            let temp_base = session.ensure_temp_base()?;
            let in_flight = session.in_flight();
            let token = CancellationToken::new();
            in_flight
                .lock()
                .expect("in_flight mutex")
                .insert(id.to_string(), token.clone());
            debug!(id, %fqbn, libs = lib_dirs.len(), "compile: spawning");

            tokio::spawn(run_compile(
                session.daemon(),
                responder.clone(),
                in_flight,
                temp_base,
                token,
                fqbn,
                opts,
                source,
                lib_dirs,
            ));
        }
        // `cancel`'s envelope `id` is the in-flight request's id (per the
        // protocol). Fire its token; the task emits the terminal `error{cancelled}`.
        RequestBody::Cancel {} => {
            if let Some(token) = session.in_flight().lock().expect("in_flight mutex").get(id) {
                debug!(id, "cancel: signalling in-flight request");
                token.cancel();
            } else {
                debug!(id, "cancel: no in-flight request for id");
            }
        }
        // Long-running and cancellable like `compile`: spawn so the read loop
        // stays responsive, and own the terminal on the spawned task.
        RequestBody::Upload {
            fqbn,
            port,
            upload_speed,
            artifact,
        } => {
            let in_flight = session.in_flight();
            let token = CancellationToken::new();
            in_flight
                .lock()
                .expect("in_flight mutex")
                .insert(id.to_string(), token.clone());
            debug!(id, %fqbn, %port, "upload: spawning");

            tokio::spawn(run_upload(
                session.daemon(),
                responder.clone(),
                in_flight,
                token,
                fqbn,
                port,
                upload_speed,
                artifact,
                // An ordinary upload's artifact already lives in arduino-cli's own temp build dir
                // (see `run_compile`/`compile_stream`), so there is nothing to stage.
                None,
            ));
        }
        // Same pump as `upload`, differing only in where the image comes from: the resource root
        // rather than the artifact a compile just produced.
        RequestBody::FlashFirmware {
            fqbn,
            port,
            upload_speed,
            pack,
            file,
        } => {
            let path = session
                .resource_root()
                .resolve_firmware_file(&pack, &file)?;
            // Stage a copy outside the resource root before handing arduino-cli the path — see
            // `stage_firmware_image`'s doc for why flashing must never write into the pack's own
            // directory. `Artifact` carries only `.path` here (the wire shape is unused for a
            // helper-resolved image); it stays `Artifact` rather than a bare path because
            // `run_upload`'s signature is shared with `upload`, whose artifact really does arrive
            // over the wire with the other fields populated.
            let staged = stage_firmware_image(&path)?;
            let staged_path = staged.path().join(
                path.file_name()
                    .expect("resolve_firmware_file returns a file path"),
            );
            let artifact = Artifact {
                format: "bin".into(),
                path: staged_path.to_string_lossy().into_owned(),
                data: None,
                parts: Vec::new(),
            };

            let in_flight = session.in_flight();
            let token = CancellationToken::new();
            in_flight
                .lock()
                .expect("in_flight mutex")
                .insert(id.to_string(), token.clone());
            debug!(id, %fqbn, %port, %pack, %file, "flashFirmware: spawning");

            tokio::spawn(run_upload(
                session.daemon(),
                responder.clone(),
                in_flight,
                token,
                fqbn,
                port,
                upload_speed,
                artifact,
                // Held until the upload finishes: dropping it earlier would remove the staged copy
                // (and arduino-cli's `*_flashed.bin` siblings alongside it) mid-flash.
                Some(staged),
            ));
        }
        // Open the bidirectional monitor stream, confirm the port opened, then keep
        // a pump task streaming `monitorData` under *this* (the open) request's id.
        // The session owns the live monitor; `monitorWrite`/`monitorClose` reach it
        // by id-independent session state, not by their own ids.
        RequestBody::MonitorOpen { port, baud_rate } => {
            if session.has_monitor() {
                return Err(Error::InvalidRequest(
                    "a monitor is already open; close it before opening another".into(),
                ));
            }
            open_monitor(session, responder, &port, baud_rate).await?;
        }
        // Push serial bytes into the open monitor's outbound stream. No reply (per
        // the protocol). A dropped channel means the monitor just closed.
        RequestBody::MonitorWrite { data } => {
            let Some(cmd_tx) = session.monitor_cmd_tx() else {
                return Err(Error::InvalidRequest(
                    "monitorWrite with no monitor open".into(),
                ));
            };
            if cmd_tx
                .send(MonitorCommand::Write(data.into_bytes()))
                .await
                .is_err()
            {
                warn!(id, "monitorWrite dropped: monitor stream closed");
            }
        }
        // Close the open monitor and acknowledge. Idempotent: closing when none is
        // open still returns `result {}`.
        RequestBody::MonitorClose {} => {
            session.close_monitor().await;
            debug!(id, "monitorClose: monitor closed");
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        // Long-running and cancellable like `compile`. An install mutates the
        // shared data dir and ends with a daemon reinit, so it is serialized
        // daemon-wide: the owned lock guard rides in the spawned task and
        // releases on completion.
        RequestBody::InstallPlatform { platform, version } => {
            let (package, architecture) = platform
                .split_once(':')
                .filter(|(package, arch)| !package.is_empty() && !arch.is_empty())
                .map(|(package, arch)| (package.to_string(), arch.to_string()))
                .ok_or_else(|| {
                    Error::InvalidRequest(format!(
                        "installPlatform requires a `vendor:architecture` platform id, got {platform:?}"
                    ))
                })?;
            let daemon = session.daemon();
            let Ok(install_guard) = daemon.install_lock().try_lock_owned() else {
                return Err(Error::InvalidRequest(
                    "a platform install is already running".into(),
                ));
            };
            let in_flight = session.in_flight();
            let token = CancellationToken::new();
            in_flight
                .lock()
                .expect("in_flight mutex")
                .insert(id.to_string(), token.clone());
            debug!(id, %platform, "installPlatform: spawning");

            tokio::spawn(run_install(
                daemon,
                responder.clone(),
                in_flight,
                token,
                install_guard,
                package,
                architecture,
                version.unwrap_or_default(),
            ));
        }
    }

    Ok(())
}

/// Open the monitor: dial the bidi stream, await the leading `Opened` event, and on
/// success reply `result {}`, install the live monitor on the session, and spawn the
/// pump that streams `monitorData` (and a terminal `error` on stream death) under
/// the open request's id.
async fn open_monitor(
    session: &mut Session,
    responder: &Responder,
    port: &str,
    baud_rate: u32,
) -> Result<()> {
    let id = responder.id();
    let (cmd_tx, cmd_rx) = mpsc::channel::<MonitorCommand>(MONITOR_COMMAND_CAPACITY);

    let mut client = session.daemon().client();
    // Heap-pin so the stream can move into the pump task after the open handshake.
    let mut stream = Box::pin(client.monitor(port, baud_rate, cmd_rx).await?);

    // The first event confirms the port opened (or reports why it didn't).
    match stream.next().await {
        Some(Ok(MonitorEvent::Opened)) => {}
        Some(Ok(MonitorEvent::Error(message))) => return Err(Error::Daemon(message)),
        Some(Ok(MonitorEvent::Data(_))) => {
            return Err(Error::Daemon(
                "monitor streamed data before confirming the port opened".into(),
            ));
        }
        Some(Err(e)) => return Err(e),
        None => return Err(Error::Daemon("monitor stream closed before opening".into())),
    }

    debug!(id, %port, baud_rate, "monitorOpen: port opened");
    responder
        .send(ResponseBody::Result(serde_json::json!({})))
        .await;

    // Detach the pump: it owns the (heap-pinned) stream for the monitor's life,
    // emitting `monitorData` until a close, an error, or the stream ending.
    let pump_responder = responder.clone();
    let task = tokio::spawn(async move {
        monitor_pump(stream, &pump_responder).await;
    });
    session.set_monitor(MonitorSession { cmd_tx, task });
    Ok(())
}

/// Pump inbound monitor events to the browser as `monitorData`. A port error
/// surfaces as a terminal `error` under the open request's id; a clean stream end
/// (after `monitorClose`) is silent, the close's own `result {}` having replied.
async fn monitor_pump(
    mut stream: impl futures::Stream<Item = Result<MonitorEvent>> + Unpin,
    responder: &Responder,
) {
    while let Some(event) = stream.next().await {
        match event {
            Ok(MonitorEvent::Data(data)) => {
                responder.send(ResponseBody::MonitorData { data }).await;
            }
            // A second `Opened` is not expected once the port is up; ignore it
            // rather than treat a benign duplicate as fatal.
            Ok(MonitorEvent::Opened) => {}
            Ok(MonitorEvent::Error(message)) => {
                return responder.send_error(&Error::Daemon(message)).await;
            }
            Err(e) => return responder.send_error(&e).await,
        }
    }
}

/// Drive one `compile` to its terminal reply, then deregister from `in_flight`.
/// Owns every terminal for its request id (`result`, `error`, `error{cancelled}`).
#[allow(clippy::too_many_arguments)]
async fn run_compile(
    daemon: Arc<Daemon>,
    responder: Responder,
    in_flight: InFlight,
    temp_base: Arc<TempDir>,
    token: CancellationToken,
    fqbn: String,
    opts: CompileOptions,
    source: String,
    lib_dirs: Vec<PathBuf>,
) {
    let id = responder.id().to_string();
    compile_stream(
        &daemon, &responder, &temp_base, &token, &fqbn, &opts, &source, &lib_dirs,
    )
    .await;
    in_flight.lock().expect("in_flight mutex").remove(&id);
}

/// The cancellable compile pump. Materializes the sketch, opens the stream, and
/// translates each event into a WS response until a terminal one is sent.
#[allow(clippy::too_many_arguments)]
pub async fn compile_stream(
    daemon: &Daemon,
    responder: &Responder,
    temp_base: &TempDir,
    token: &CancellationToken,
    fqbn: &str,
    opts: &CompileOptions,
    source: &str,
    lib_dirs: &[PathBuf],
) {
    let sketch_dir = match write_sketch(temp_base.path(), responder.id(), source) {
        Ok(dir) => dir,
        Err(e) => return responder.send_error(&e).await,
    };

    let mut client = daemon.client();
    let stream = match client.compile(fqbn, &sketch_dir, opts, lib_dirs).await {
        Ok(stream) => stream,
        Err(e) => return responder.send_error(&e).await,
    };
    tokio::pin!(stream);

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                return responder
                    .send(ResponseBody::Error {
                        code: Error::Cancelled.code().into(),
                        message: Error::Cancelled.to_string(),
                    })
                    .await;
            }
            item = stream.next() => match item {
                Some(Ok(CompileEvent::Log(chunk))) => {
                    responder.send(ResponseBody::Log { chunk }).await;
                }
                Some(Ok(CompileEvent::Progress { phase, percent })) => {
                    responder.send(ResponseBody::Progress { phase, percent }).await;
                }
                Some(Ok(CompileEvent::Done(artifact))) => {
                    return responder
                        .send(ResponseBody::Result(
                            serde_json::to_value(CompileResult { artifact })
                                .expect("CompileResult always serializes"),
                        ))
                        .await;
                }
                Some(Err(e)) => return responder.send_error(&e).await,
                // Stream ended without a `Done`: no artifact was produced.
                None => {
                    return responder
                        .send_error(&Error::Daemon(
                            "compile ended without producing an artifact".into(),
                        ))
                        .await;
                }
            },
        }
    }
}

/// Drive one `upload` to its terminal reply, then deregister from `in_flight`.
/// Owns every terminal for its request id (`result`, `error`, `error{cancelled}`).
#[allow(clippy::too_many_arguments)]
async fn run_upload(
    daemon: Arc<Daemon>,
    responder: Responder,
    in_flight: InFlight,
    token: CancellationToken,
    fqbn: String,
    port: String,
    upload_speed: u32,
    artifact: Artifact,
    // `flashFirmware`'s staged copy (see `stage_firmware_image`), kept alive for the whole flash so
    // it (and arduino-cli's `*_flashed.bin` siblings) is removed only once the upload is done.
    // `None` for an ordinary `upload`, whose artifact needs no staging.
    _firmware_temp: Option<TempDir>,
) {
    let id = responder.id().to_string();
    upload_stream(
        &daemon,
        &responder,
        &token,
        &fqbn,
        &port,
        upload_speed,
        &artifact,
    )
    .await;
    in_flight.lock().expect("in_flight mutex").remove(&id);
}

/// Copy a resolved firmware image and its siblings (bootloader, partition table) into a fresh temp
/// directory, so `flashFirmware` never points arduino-cli straight at a file inside the resource
/// root. arduino-cli/esptool write `*_flashed.bin` siblings next to whatever they flash (observed on
/// hardware: `telemetrix-ble.ino_flashed.bin`, `.bootloader_flashed.bin`, `.partitions_flashed.bin`,
/// and `boot_app0_flashed.bin`, which does not even come from the pack). Left pointed at the pack's
/// own directory, that write fails outright on an install directory that is not user-writable —
/// exactly the stuck state this feature exists to remove — and on macOS it lands inside the `.app`
/// bundle and invalidates its signature.
///
/// The whole containing directory is copied, not just the named image, because arduino-cli's esp32
/// upload recipe finds the bootloader and partition table by name next to the app image.
pub fn stage_firmware_image(image: &Path) -> Result<TempDir> {
    let src_dir = image.parent().ok_or_else(|| {
        Error::Resource(format!(
            "firmware image {} has no containing directory",
            image.display()
        ))
    })?;
    let staged = TempDir::new("thingblock-link-firmware")?;
    for entry in std::fs::read_dir(src_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), staged.path().join(entry.file_name()))?;
        }
    }
    Ok(staged)
}

/// The cancellable upload pump. Opens the stream and translates each event into a
/// WS response until a terminal one is sent. Upload has no structured progress, so
/// only `log` chunks flow before the terminal `result`.
#[allow(clippy::too_many_arguments)]
async fn upload_stream(
    daemon: &Daemon,
    responder: &Responder,
    token: &CancellationToken,
    fqbn: &str,
    port: &str,
    upload_speed: u32,
    artifact: &Artifact,
) {
    let mut client = daemon.client();
    let stream = match client
        .upload(fqbn, &artifact.path, port, upload_speed)
        .await
    {
        Ok(stream) => stream,
        Err(e) => return responder.send_error(&e).await,
    };
    tokio::pin!(stream);

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                return responder
                    .send(ResponseBody::Error {
                        code: Error::Cancelled.code().into(),
                        message: Error::Cancelled.to_string(),
                    })
                    .await;
            }
            item = stream.next() => match item {
                Some(Ok(UploadEvent::Log(chunk))) => {
                    responder.send(ResponseBody::Log { chunk }).await;
                }
                Some(Ok(UploadEvent::Done)) => {
                    return responder
                        .send(ResponseBody::Result(serde_json::json!({})))
                        .await;
                }
                Some(Err(e)) => return responder.send_error(&e).await,
                // Stream ended without a `Done`: the flash did not complete.
                None => {
                    return responder
                        .send_error(&Error::Daemon(
                            "upload ended without completing".into(),
                        ))
                        .await;
                }
            },
        }
    }
}

/// Drive one `installPlatform` to its terminal reply, then deregister from
/// `in_flight`. Holds the daemon-wide install guard for its whole run. Owns
/// every terminal for its request id (`result`, `error`, `error{cancelled}`).
#[allow(clippy::too_many_arguments)]
async fn run_install(
    daemon: Arc<Daemon>,
    responder: Responder,
    in_flight: InFlight,
    token: CancellationToken,
    _install_guard: tokio::sync::OwnedMutexGuard<()>,
    package: String,
    architecture: String,
    version: String,
) {
    let id = responder.id().to_string();
    install_stream(
        &daemon,
        &responder,
        &token,
        &package,
        &architecture,
        &version,
    )
    .await;
    in_flight.lock().expect("in_flight mutex").remove(&id);
}

/// The cancellable install pump. Opens the stream and translates each event into
/// a WS response until a terminal one is sent. On success the daemon instance is
/// reinitialized first — it does not see the newly installed core until `Init`
/// re-runs, and the next compile would fail without it.
async fn install_stream(
    daemon: &Daemon,
    responder: &Responder,
    token: &CancellationToken,
    package: &str,
    architecture: &str,
    version: &str,
) {
    let mut client = daemon.client();
    let stream = match client
        .platform_install(package, architecture, version)
        .await
    {
        Ok(stream) => stream,
        Err(e) => return responder.send_error(&e).await,
    };
    tokio::pin!(stream);

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                return responder
                    .send(ResponseBody::Error {
                        code: Error::Cancelled.code().into(),
                        message: Error::Cancelled.to_string(),
                    })
                    .await;
            }
            item = stream.next() => match item {
                Some(Ok(PlatformInstallEvent::Log(chunk))) => {
                    responder.send(ResponseBody::Log { chunk }).await;
                }
                Some(Ok(PlatformInstallEvent::Progress { phase, percent })) => {
                    responder.send(ResponseBody::Progress { phase, percent }).await;
                }
                Some(Ok(PlatformInstallEvent::Done)) => {
                    return match daemon.reinit().await {
                        Ok(()) => {
                            responder
                                .send(ResponseBody::Result(serde_json::json!({})))
                                .await;
                        }
                        Err(e) => responder.send_error(&e).await,
                    };
                }
                Some(Err(e)) => return responder.send_error(&e).await,
                // Stream ended without a `Done`: the install did not complete.
                None => {
                    return responder
                        .send_error(&Error::Daemon(
                            "platform install ended without completing".into(),
                        ))
                        .await;
                }
            },
        }
    }
}

/// Materialize `source` as a sketch under `base`. arduino-cli compiles a sketch
/// *directory* whose main `.ino` shares the folder's name, so we create
/// `<base>/<name>/<name>.ino`, keyed by the request id (sanitized for the FS).
fn write_sketch(base: &Path, id: &str, source: &str) -> Result<PathBuf> {
    let name = sketch_name(id);
    let dir = base.join(&name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{name}.ino")), source)?;
    Ok(dir)
}

/// A filesystem-safe sketch folder name for a request id. The `sketch_` prefix
/// guards against ids that would otherwise be empty or lead with a digit.
fn sketch_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("sketch_{safe}")
}
