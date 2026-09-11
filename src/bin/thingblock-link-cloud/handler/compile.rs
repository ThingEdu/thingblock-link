//! `POST /compile` controller.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use thingblock_link::error::{Error, Result};
use thingblock_link::server::protocol::{
    Artifact, ArtifactPart, CompileOptions, CompileResult, LibRef, ResponseBody,
};
use thingblock_link::service::arduino::bridge::{Responder, compile_stream};
use thingblock_link::utils::tempdir::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::routes::AppState;

const RESPONSE_CHANNEL_CAPACITY: usize = 64;

/// Distinguishes concurrent compiles in the logs; never leaves the process.
fn next_request_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("compile-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileRequest {
    fqbn: String,
    #[serde(default)]
    options: CompileOptions,
    source: String,
    #[serde(default)]
    libs: Vec<LibRef>,
}

pub async fn compile(
    State(state): State<AppState>,
    Json(req): Json<CompileRequest>,
) -> Result<Response> {
    // Before streaming starts, so a bad ref is a 400 rather than an in-band
    // error after the headers are sent.
    let lib_dirs = req
        .libs
        .iter()
        .map(|r| state.resource_root.resolve_lib_dir(&r.pack, &r.lib))
        .collect::<Result<Vec<_>>>()?;

    let temp_base = TempDir::new("thingblock-compile")?;
    let (tx, rx) = mpsc::channel(RESPONSE_CHANNEL_CAPACITY);
    let token = CancellationToken::new();

    let daemon = state.daemon.clone();
    let task_token = token.clone();
    let responder = Responder::new(next_request_id(), tx);
    tokio::spawn(async move {
        compile_stream(
            &daemon,
            &responder,
            &temp_base,
            &task_token,
            &req.fqbn,
            &req.options,
            &req.source,
            &lib_dirs,
        )
        .await;
    });

    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(ndjson(rx, token.drop_guard())),
    )
        .into_response())
}

/// One JSON object per line, without the request `id` — one compile per
/// connection leaves nothing to correlate. The drop guard rides in the stream
/// state so an aborted request cancels the compile.
fn ndjson(
    rx: mpsc::Receiver<thingblock_link::server::protocol::Response>,
    guard: DropGuard,
) -> impl futures::Stream<Item = std::io::Result<String>> {
    futures::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        let response = rx.recv().await?;
        let body = with_artifact_bytes(response.body);
        let line = format!("{}\n", serde_json::to_string(&body).ok()?);
        Some((Ok(line), (rx, guard)))
    })
}

/// A browser can't read the server's filesystem, so ship bytes, not a path.
fn with_artifact_bytes(body: ResponseBody) -> ResponseBody {
    let ResponseBody::Result(value) = &body else {
        return body;
    };
    let Ok(result) = serde_json::from_value::<CompileResult>(value.clone()) else {
        return body;
    };

    let mut parts = Vec::with_capacity(result.artifact.parts.len());
    for part in result.artifact.parts {
        match std::fs::read(&part.path) {
            Ok(bytes) => parts.push(ArtifactPart {
                offset: part.offset,
                path: String::new(),
                data: Some(BASE64.encode(bytes)),
            }),
            Err(e) => return error_body(&Error::Io(e)),
        }
    }

    // The app image alone, for single-image targets and as the `parts` fallback.
    let bytes = match std::fs::read(&result.artifact.path) {
        Ok(bytes) => bytes,
        Err(e) => return error_body(&Error::Io(e)),
    };
    let artifact = Artifact {
        format: result.artifact.format,
        path: String::new(),
        data: Some(BASE64.encode(bytes)),
        parts,
    };
    ResponseBody::Result(
        serde_json::to_value(CompileResult { artifact }).expect("CompileResult always serializes"),
    )
}

fn error_body(error: &Error) -> ResponseBody {
    ResponseBody::Error {
        code: error.code().into(),
        message: error.to_string(),
    }
}
