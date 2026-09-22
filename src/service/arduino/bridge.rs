//! The only place the WS and arduino-cli schemas meet; neither leaks past it.

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

const MONITOR_COMMAND_CAPACITY: usize = 32;

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

    pub async fn send(&self, body: ResponseBody) {
        let response = Response {
            id: self.id.clone(),
            body,
        };
        if self.tx.send(response).await.is_err() {
            warn!(id = %self.id, "response dropped: ws writer closed");
        }
    }

    async fn send_error(&self, error: &Error) {
        self.send(ResponseBody::Error {
            code: error.code().into(),
            message: error.to_string(),
        })
        .await;
    }
}

/// `Err` becomes a terminal `error`; handlers that own their terminal reply return `Ok`.
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
        // The daemon is connectionless per-port, so `connect` only records the port.
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
            session.close_monitor().await;
            debug!(id, "disconnect: cleared selected port");
            session.clear_port();
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        // Spawned so the read loop stays responsive to `cancel`; the task owns its terminal.
        RequestBody::Compile {
            fqbn,
            options,
            source,
            libs,
        } => {
            let opts: CompileOptions = serde_json::from_value(options)
                .map_err(|e| Error::InvalidRequest(format!("compile options: {e}")))?;
            // Resolve lib refs before registering the task so a bad ref fails fast.
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
        // Per the protocol, `cancel`'s envelope `id` is the in-flight request's id.
        RequestBody::Cancel {} => {
            if let Some(token) = session.in_flight().lock().expect("in_flight mutex").get(id) {
                debug!(id, "cancel: signalling in-flight request");
                token.cancel();
            } else {
                debug!(id, "cancel: no in-flight request for id");
            }
        }
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
                None,
            ));
        }
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
            // Staged so arduino-cli never writes into the pack's directory (see `stage_firmware_image`).
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
                // Dropping it before the upload finishes would delete the image mid-flash.
                Some(staged),
            ));
        }
        // `monitorWrite`/`monitorClose` reach the monitor via session state, not by request id.
        RequestBody::MonitorOpen { port, baud_rate } => {
            if session.has_monitor() {
                return Err(Error::InvalidRequest(
                    "a monitor is already open; close it before opening another".into(),
                ));
            }
            open_monitor(session, responder, &port, baud_rate).await?;
        }
        // No reply, per the protocol.
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
        RequestBody::MonitorClose {} => {
            session.close_monitor().await;
            debug!(id, "monitorClose: monitor closed");
            responder
                .send(ResponseBody::Result(serde_json::json!({})))
                .await;
        }
        // Serialized daemon-wide: an install mutates the shared data dir and ends with a reinit.
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

async fn open_monitor(
    session: &mut Session,
    responder: &Responder,
    port: &str,
    baud_rate: u32,
) -> Result<()> {
    let id = responder.id();
    let (cmd_tx, cmd_rx) = mpsc::channel::<MonitorCommand>(MONITOR_COMMAND_CAPACITY);

    let mut client = session.daemon().client();
    let mut stream = Box::pin(client.monitor(port, baud_rate, cmd_rx).await?);

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
            Ok(MonitorEvent::Opened) => {}
            Ok(MonitorEvent::Error(message)) => {
                return responder.send_error(&Error::Daemon(message)).await;
            }
            Err(e) => return responder.send_error(&e).await,
        }
    }
}

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
    // Keeps `flashFirmware`'s staged copy alive until the flash completes.
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

/// esptool writes `*_flashed.bin` beside its inputs (read-only install dir, signed macOS `.app`); copies siblings too.
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

/// On success, reinit the daemon first: it doesn't see the new core until `Init` re-runs.
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

/// arduino-cli compiles a sketch directory whose main `.ino` shares the folder's name.
fn write_sketch(base: &Path, id: &str, source: &str) -> Result<PathBuf> {
    let name = sketch_name(id);
    let dir = base.join(&name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{name}.ino")), source)?;
    Ok(dir)
}

/// The `sketch_` prefix guards against ids that are empty or lead with a digit.
fn sketch_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("sketch_{safe}")
}
