//! The arduino-cli service: spawns and owns the arduino-cli daemon process, holds its gRPC
//! channel, and runs the `Create`/`Init` handshake once for the instance id every other RPC
//! needs. The per-RPC translations live in [`grpc`].

pub mod grpc;
pub mod model;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tonic::transport::Channel;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use grpc::{Client, cli};

/// How long to wait for the freshly spawned daemon to start accepting gRPC.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Handle to the running arduino-cli daemon and its initialized gRPC instance.
pub struct ArduinoService {
    /// The daemon process, killed when this service drops.
    _child: Child,
    /// The daemon's stdin; it stays open while we run and closes when we die, which ends the daemon.
    _stdin: ChildStdin,
    channel: Channel,
    instance: cli::Instance,
    /// Serializes platform installs across sessions.
    install_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl ArduinoService {
    /// [`Self::start_with`] without a config dir: arduino-cli uses its defaults
    /// (`~/.arduino15`). Used by tests.
    pub async fn start(cli_path: Option<PathBuf>) -> Result<Self> {
        Self::start_with(cli_path, None).await
    }

    /// Spawns `arduino-cli daemon` on a free port, dials it and runs `Create` + `Init`. With
    /// `config_dir`, the daemon uses its `arduino-cli.yaml` and runs with it as cwd.
    pub async fn start_with(
        cli_path: Option<PathBuf>,
        config_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let cli_path = resolve_cli_path(cli_path);
        let port = pick_free_port()?;
        info!(binary = %cli_path.display(), port, config_dir = ?config_dir, "starting arduino-cli daemon");

        let mut command = Command::new(&cli_path);
        command
            .arg("daemon")
            .arg("--port")
            .arg(port.to_string());
        if let Some(dir) = config_dir {
            let config_file = dir.join("arduino-cli.yaml");
            if !config_file.is_file() {
                return Err(Error::Arduino(format!(
                    "config file not found: {}",
                    config_file.display()
                )));
            }
            command
                .arg("--config-file")
                .arg(&config_file)
                .current_dir(&dir);
        }
        // arduino-cli exits when its stdin closes. We hold the write end for as long as we live, so
        // the daemon also goes when this process is killed outright and `kill_on_drop` never runs.
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Arduino(format!("spawn {}: {e}", cli_path.display())))?;

        let stdin = child.stdin.take().expect("stdin is piped");
        forward_daemon_output(&mut child);

        let channel = connect_with_retry(port).await?;
        let instance = handshake(channel.clone()).await?;
        info!(instance = instance.id, "arduino-cli daemon ready");

        Ok(Self {
            _child: child,
            _stdin: stdin,
            channel,
            instance,
            install_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Re-runs `Init` to reload the daemon's data dir. Must run after a platform install.
    pub async fn reinit(&self) -> Result<()> {
        let mut client =
            cli::arduino_core_service_client::ArduinoCoreServiceClient::new(self.channel.clone());
        drain_init(&mut client, self.instance).await
    }

    /// The shared platform-install lock.
    pub fn install_lock(&self) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        self.install_lock.clone()
    }

    pub fn channel(&self) -> Channel {
        self.channel.clone()
    }

    pub fn instance(&self) -> &cli::Instance {
        &self.instance
    }

    /// A ready-to-use gRPC client bound to this daemon's instance.
    pub fn client(&self) -> Client {
        Client::new(self.channel.clone(), self.instance)
    }
}

/// The arduino-cli to run: the caller's override when packaged, else the in-tree dev default.
fn resolve_cli_path(override_path: Option<PathBuf>) -> PathBuf {
    override_path.unwrap_or_else(bundled_cli_path)
}

/// Dev default config dir: the crate root, where `arduino-cli.yaml` and the `data/` bundle live.
/// Packaged builds pass `--config-dir` instead.
pub fn default_config_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Dev default: the arduino-cli vendored in `arduino-cli-binaries/`. Packaged builds pass
/// `--arduino-cli` instead.
fn bundled_cli_path() -> PathBuf {
    let (dir, exe) = if cfg!(target_os = "windows") {
        ("arduino-cli_win_64bit", "arduino-cli.exe")
    } else if cfg!(target_os = "macos") {
        ("arduino-cli_mac_arm64", "arduino-cli")
    } else if cfg!(target_arch = "aarch64") {
        ("arduino-cli_linux_arm64", "arduino-cli")
    } else {
        ("arduino-cli_linux_64bit", "arduino-cli")
    };

    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("arduino-cli-binaries")
        .join(dir)
        .join(exe)
}

/// Grabs an OS-assigned free loopback port and releases it for the daemon to bind.
fn pick_free_port() -> Result<u16> {
    let free_port_err = |e| Error::Arduino(format!("pick free daemon port: {e}"));
    let listener = TcpListener::bind("127.0.0.1:0").map_err(free_port_err)?;
    Ok(listener.local_addr().map_err(free_port_err)?.port())
}

/// Pumps the daemon's stdout/stderr into `tracing`.
fn forward_daemon_output(child: &mut Child) {
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(target: "arduino_cli_daemon", "{line}");
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "arduino_cli_daemon", "{line}");
            }
        });
    }
}

/// Dials the daemon, retrying until it's listening or `CONNECT_TIMEOUT` elapses.
async fn connect_with_retry(port: u16) -> Result<Channel> {
    let endpoint = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .map_err(|e| Error::Arduino(format!("invalid daemon endpoint: {e}")))?;

    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match endpoint.connect().await {
            Ok(channel) => return Ok(channel),
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(Error::Arduino(format!(
                        "daemon not reachable on port {port}: {e}"
                    )));
                }
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
        }
    }
}

/// `Create`s a core instance, then runs `Init` until the daemon reports ready.
async fn handshake(channel: Channel) -> Result<cli::Instance> {
    let mut client = cli::arduino_core_service_client::ArduinoCoreServiceClient::new(channel);

    let instance = client
        .create(cli::CreateRequest {})
        .await?
        .into_inner()
        .instance
        .ok_or_else(|| Error::Arduino("Create returned no instance".into()))?;

    drain_init(&mut client, instance).await?;
    Ok(instance)
}

/// Runs `Init` on `instance` and drains its stream. Per-item errors (e.g. a missing index) are
/// only logged.
async fn drain_init(
    client: &mut cli::arduino_core_service_client::ArduinoCoreServiceClient<Channel>,
    instance: cli::Instance,
) -> Result<()> {
    let mut init = client
        .init(cli::InitRequest {
            instance: Some(instance),
            profile: String::new(),
            sketch_path: String::new(),
        })
        .await?
        .into_inner();

    while let Some(resp) = init.message().await? {
        match resp.message {
            Some(cli::init_response::Message::Error(status)) => {
                warn!(code = status.code, message = %status.message, "daemon init error");
            }
            Some(cli::init_response::Message::InitProgress(_))
            | Some(cli::init_response::Message::Profile(_)) => {}
            None => {}
        }
    }

    Ok(())
}
