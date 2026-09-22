//! Translates WS requests into arduino-cli gRPC calls and streams results back as responses.
//! The only place the WS and arduino-cli schemas meet, so neither leaks past it.

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

/// How many outbound monitor commands (writes/close) may queue before backpressure applies.
/// Editor serial writes are small and infrequent, so this stays small.
const MONITOR_COMMAND_CAPACITY: usize = 32;

/// Sends one request's responses to the session's writer, stamping each with the request `id`.
/// Cloneable so a streaming handler can emit many `log`/`progress` before its terminal reply.
#[derive(Clone)]
pub struct Responder {
    id: String,
    tx: mpsc::Sender<Response>,
}

impl Responder {
    pub fn new(id: String, tx: mpsc::Sender<Response>) -> Self {
        Self { id, tx }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Sends one response body for this request. A closed channel means the browser is gone,
    /// which is not actionable here, so it is logged and dropped.
    pub async fn send(&self, body: ResponseBody) {
        let response = Response {
            id: self.id.clone(),
            body,
        };
        if self.tx.send(response).await.is_err() {
            warn!(id = %self.id, "response dropped: ws writer closed");
        }
    }

    /// Sends a terminal `error` built from an [`Error`]'s code and message.
    async fn send_error(&self, error: &Error) {
        self.send(ResponseBody::Error {
            code: error.code().into(),
            message: error.to_string(),
        })
        .await;
    }
}

/// Routes one request to its gRPC translation, streaming responses back through `responder`.
/// Each request gets exactly one terminal: return `Err` (caller sends `error`) or send it + `Ok`.
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
        // The daemon is connectionless per-port, so `connect` only records the chosen port;
        // whether it exists is checked later by upload/monitor.
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
            // Close any open monitor first so the port is released before it is cleared.
            session.close_monitor().await;
            debug!(id, "disconnect: cleared selected port");
            session.clear_port();
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        // Long-running and cancellable: spawned so the read loop stays responsive to `cancel`.
        // The spawned task owns the terminal reply.
        RequestBody::Compile {
            fqbn,
            options,
            source,
            libs,
        } => {
            let opts: CompileOptions = serde_json::from_value(options)
                .map_err(|e| Error::InvalidRequest(format!("compile options: {e}")))?;
            // Resolve vendored lib refs against the served root before registering the task,
            // so a bad ref fails fast as `error{resource}` instead of spawning a doomed compile.
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
        // Per the protocol, `cancel`'s envelope `id` is the in-flight request's id. Firing its
        // token makes that task emit the terminal `error{cancelled}`.
        RequestBody::Cancel {} => {
            if let Some(token) = session.in_flight().lock().expect("in_flight mutex").get(id) {
                debug!(id, "cancel: signalling in-flight request");
                token.cancel();
            } else {
                debug!(id, "cancel: no in-flight request for id");
            }
        }
        // Spawned like `compile` so it stays cancellable; the task owns the terminal reply.
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
                // A compiled artifact already lives in arduino-cli's temp build dir: no staging.
                None,
            ));
        }
        // Same pump as `upload`, but the image comes from the resource root, not a compile.
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
            // Stage a copy so arduino-cli never writes into the pack's directory (see
            // `stage_firmware_image`). Only `.path` matters; `Artifact` is shared with `upload`.
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
                // Held until the upload finishes: dropping it earlier would delete the staged copy
                // (and arduino-cli's `*_flashed.bin` siblings) mid-flash.
                Some(staged),
            ));
        }
        // Opens the serial monitor; its `monitorData` streams under this open request's id.
        // `monitorWrite`/`monitorClose` reach the monitor via session state, not by request id.
        RequestBody::MonitorOpen { port, baud_rate } => {
            if session.has_monitor() {
                return Err(Error::InvalidRequest(
                    "a monitor is already open; close it before opening another".into(),
                ));
            }
            open_monitor(session, responder, &port, baud_rate).await?;
        }
        // Pushes serial bytes into the open monitor; no reply, per the protocol.
        // A send failure means the monitor just closed.
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
        // Idempotent: closing when no monitor is open still replies `result {}`.
        RequestBody::MonitorClose {} => {
            session.close_monitor().await;
            debug!(id, "monitorClose: monitor closed");
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        // Spawned and cancellable like `compile`, but serialized daemon-wide: an install mutates
        // the shared data dir and ends with a reinit. The owned lock guard rides in the task.
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

/// Opens the monitor stream, confirms the port opened, replies `result {}`, and spawns the pump.
/// The pump streams `monitorData` under the open request's id for the monitor's lifetime.
async fn open_monitor(
    session: &mut Session,
    responder: &Responder,
    port: &str,
    baud_rate: u32,
) -> Result<()> {
    let id = responder.id();
    let (cmd_tx, cmd_rx) = mpsc::channel::<MonitorCommand>(MONITOR_COMMAND_CAPACITY);

    let mut client = session.daemon().client();
    // Heap-pinned so the stream can move into the pump task after the open handshake.
    let mut stream = Box::pin(client.monitor(port, baud_rate, cmd_rx).await?);

    // The first event confirms the port opened, or reports why it didn't.
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

    let pump_responder = responder.clone();
    let task = tokio::spawn(async move {
        monitor_pump(stream, &pump_responder).await;
    });
    session.set_monitor(MonitorSession { cmd_tx, task });
    Ok(())
}

/// Forwards inbound monitor data as `monitorData`; a port error ends it with a terminal `error`.
/// A clean stream end is silent: `monitorClose` already sent the `result {}`.
async fn monitor_pump(
    mut stream: impl futures::Stream<Item = Result<MonitorEvent>> + Unpin,
    responder: &Responder,
) {
    while let Some(event) = stream.next().await {
        match event {
            Ok(MonitorEvent::Data(data)) => {
                responder.send(ResponseBody::MonitorData { data }).await;
            }
            // A duplicate `Opened` after the port is up is benign, not fatal.
            Ok(MonitorEvent::Opened) => {}
            Ok(MonitorEvent::Error(message)) => {
                return responder.send_error(&Error::Daemon(message)).await;
            }
            Err(e) => return responder.send_error(&e).await,
        }
    }
}

/// Runs one spawned `compile` to its terminal reply, then deregisters it from `in_flight`
/// so a later `cancel` for this id finds nothing.
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

/// Writes the sketch, runs the compile stream, and translates each event into a WS response.
/// Sends exactly one terminal: `result`, `error`, or `error{cancelled}`.
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

/// Runs one spawned `upload`/`flashFirmware` to its terminal reply, then deregisters it
/// from `in_flight` so a later `cancel` for this id finds nothing.
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
    // Keeps `flashFirmware`'s staged copy alive until the flash completes; `None` for `upload`.
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

/// Copies the image's dir (esp32 needs bootloader/partitions beside it) to a temp dir; esptool
/// writes `*_flashed.bin` beside inputs: fails on read-only installs, breaks signed macOS `.app`s.
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

/// Runs the upload stream and translates each event into a WS response until a terminal one.
/// Upload has no structured progress, so only `log` chunks precede the terminal `result`.
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

/// Runs one spawned `installPlatform` to its terminal reply, holding the daemon-wide install
/// guard throughout, then deregisters it from `in_flight`.
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

/// Runs the install stream, translating events into WS responses. On success it reinits the
/// daemon before replying: the new core is invisible until `Init` re-runs, so compiles would fail.
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

/// Writes `source` as `<base>/<name>/<name>.ino`, keyed by request id. arduino-cli compiles a
/// sketch directory whose main `.ino` must share the folder's name.
fn write_sketch(base: &Path, id: &str, source: &str) -> Result<PathBuf> {
    let name = sketch_name(id);
    let dir = base.join(&name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{name}.ino")), source)?;
    Ok(dir)
}

/// Builds a filesystem-safe sketch folder name from a request id. The `sketch_` prefix guards
/// against ids that are empty or lead with a digit.
fn sketch_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("sketch_{safe}")
}
