use futures::Stream;
use tonic::Streaming;

use crate::error::{Error, Result};
use crate::server::protocol::PlatformStatus;
use crate::service::arduino::grpc::{Client, cli};

#[derive(Debug)]
pub enum PlatformInstallEvent {
    Log(String),
    Progress {
        phase: String,
        percent: f32,
    },
    /// The caller must reinit the daemon instance before the new platform is compilable.
    Done,
}

impl Client {
    /// An empty `query` matches all platforms.
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

    /// An empty `version` means the latest indexed release.
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

/// `phase` carries the download label across `Update` frames, which don't repeat it.
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
                    }
                    Ok(None) => return None,
                    Err(status) => return Some((Err(Error::Grpc(status)), (stream, true, phase))),
                }
            }
        },
    )
}

fn translate(
    resp: cli::PlatformInstallResponse,
    phase: &mut String,
) -> Option<Result<PlatformInstallEvent>> {
    use cli::platform_install_response::Message;

    match resp.message? {
        Message::Progress(download) => translate_download(download.message?, phase),
        Message::TaskProgress(task) => Some(Ok(PlatformInstallEvent::Progress {
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
        Message::Update(_) => None,
        Message::End(end) if end.success => (!end.message.is_empty())
            .then(|| Ok(PlatformInstallEvent::Log(format!("{}\n", end.message)))),
        Message::End(end) => Some(Err(Error::Daemon(if end.message.is_empty() {
            format!("download of {phase} failed")
        } else {
            end.message
        }))),
    }
}
