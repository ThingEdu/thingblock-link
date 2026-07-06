//! Platform (board-manager) translations: `PlatformSearch` backs the HTTP
//! `/api/platforms` install-status routes, and `PlatformInstall` backs the WS
//! `installPlatform` request that downloads a core (e.g. `esp32:esp32`) on
//! demand. The arduino-cli schema never leaks past this module — callers see
//! [`PlatformStatus`] and [`PlatformInstallEvent`].

use futures::Stream;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::server::protocol::PlatformStatus;
use crate::service::arduino::grpc::{Client, cli};

/// One translated step of a platform install, in the helper's own shapes.
#[derive(Debug)]
pub enum PlatformInstallEvent {
    /// A human-readable status line (e.g. a finished download).
    Log(String),
    /// Download / installation progress.
    Progress { phase: String, percent: f32 },
    /// Terminal success. The caller must reinit the daemon instance before the
    /// new platform is compilable (see [`crate::service::arduino::daemon::Daemon::reinit`]).
    Done,
}

impl Client {
    /// Search the package indexes for platforms matching `query` (empty for
    /// all), reporting each with its install status.
    pub async fn platform_search(&mut self, query: &str) -> Result<Vec<PlatformStatus>> {
        let instance = *self.instance();
        let response = self
            .inner()
            .platform_search(cli::PlatformSearchRequest {
                instance: Some(instance),
                search_args: query.to_string(),
                manually_installed: false,
            })
            .await?
            .into_inner();

        Ok(response.search_output.into_iter().map(to_status).collect())
    }

    /// Install (download + extract + post-install) a platform, streaming
    /// translated events. `version` empty means the latest indexed release.
    /// The stream ends after a `Done` (success) or yields an `Err` and ends.
    pub async fn platform_install(
        &mut self,
        package: &str,
        architecture: &str,
        version: &str,
    ) -> Result<impl Stream<Item = Result<PlatformInstallEvent>>> {
        let instance = *self.instance();
        let request = cli::PlatformInstallRequest {
            instance: Some(instance),
            platform_package: package.to_string(),
            architecture: architecture.to_string(),
            version: version.to_string(),
            skip_post_install: false,
            no_overwrite: false,
            skip_pre_uninstall: false,
        };

        let stream = self.inner().platform_install(request).await?.into_inner();
        Ok(into_events(stream))
    }
}

/// Map one `PlatformSummary` to the helper's [`PlatformStatus`]. The
/// human-readable name lives on a release, so prefer the latest one (falling
/// back to the installed one, then the id).
fn to_status(summary: cli::PlatformSummary) -> PlatformStatus {
    let metadata = summary.metadata.unwrap_or_default();
    let name = summary
        .releases
        .get(&summary.latest_version)
        .or_else(|| summary.releases.get(&summary.installed_version))
        .map(|release| release.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| metadata.id.clone());

    PlatformStatus {
        id: metadata.id,
        name,
        installed: !summary.installed_version.is_empty(),
        installed_version: non_empty(summary.installed_version),
        latest_version: non_empty(summary.latest_version),
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

/// Adapt the tonic `PlatformInstall` stream into a `PlatformInstallEvent`
/// stream, skipping empty frames and terminating after the first error.
/// `phase` carries the current download's label across `Update` frames, which
/// don't repeat it.
fn into_events(
    stream: Streaming<cli::PlatformInstallResponse>,
) -> impl Stream<Item = Result<PlatformInstallEvent>> {
    futures::stream::unfold(
        (stream, false, String::new()),
        |(mut stream, done, mut phase)| async move {
            if done {
                return None;
            }
            loop {
                match stream.message().await {
                    Ok(Some(resp)) => {
                        if let Some(event) = translate(resp, &mut phase) {
                            let stop = event.is_err();
                            return Some((event, (stream, stop, phase)));
                        }
                        // Empty or skippable frame; keep reading.
                    }
                    Ok(None) => return None, // stream ended cleanly
                    Err(status) => return Some((Err(Error::Grpc(status)), (stream, true, phase))),
                }
            }
        },
    )
}

/// Map one `PlatformInstallResponse` to an event, or `None` for a frame with
/// nothing to surface.
fn translate(
    resp: cli::PlatformInstallResponse,
    phase: &mut String,
) -> Option<Result<PlatformInstallEvent>> {
    use cli::platform_install_response::Message;

    match resp.message? {
        Message::Progress(download) => translate_download(download.message?, phase),
        Message::TaskProgress(task) => Some(Ok(PlatformInstallEvent::Progress {
            // `name` is the stage label; fall back to the freeform `message`.
            phase: if task.name.is_empty() {
                task.message
            } else {
                task.name
            },
            percent: task.percent,
        })),
        Message::Result(_) => Some(Ok(PlatformInstallEvent::Done)),
    }
}

/// Map one download-progress frame. A failed download is the install's error;
/// a finished one is worth a log line for the editor's details view.
fn translate_download(
    message: cli::download_progress::Message,
    phase: &mut String,
) -> Option<Result<PlatformInstallEvent>> {
    use cli::download_progress::Message;

    match message {
        Message::Start(start) => {
            *phase = start.label;
            Some(Ok(PlatformInstallEvent::Progress {
                phase: phase.clone(),
                percent: 0.0,
            }))
        }
        Message::Update(update) if update.total_size > 0 => {
            Some(Ok(PlatformInstallEvent::Progress {
                phase: phase.clone(),
                percent: 100.0 * update.downloaded as f32 / update.total_size as f32,
            }))
        }
        Message::Update(_) => None, // unknown total; nothing meaningful to report
        Message::End(end) if end.success => (!end.message.is_empty())
            .then(|| Ok(PlatformInstallEvent::Log(format!("{}\n", end.message)))),
        Message::End(end) => Some(Err(Error::Daemon(if end.message.is_empty() {
            format!("download of {phase} failed")
        } else {
            end.message
        }))),
    }
}
