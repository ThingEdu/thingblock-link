//! End-to-end check of the static resource route (Flow 1): the helper serves the
//! resource root over HTTP at `/resources` with CORS so a cross-origin editor can
//! `import()` pack files. Driven with a minimal raw-HTTP-over-TCP client to avoid
//! a heavyweight HTTP client dependency, in the same spirit as the hand-rolled
//! `TempDir`. The WS face shares the listener, so a live daemon is spun up to
//! satisfy `serve` even though these requests never reach it.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use thingblock_link::server;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::resource::ResourceRoot;
use thingblock_link::utils::tempdir::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A parsed HTTP/1.0 response: the status code, the lowercased header block, and
/// the body.
struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
}

/// Issue a one-shot `GET path` (HTTP/1.0, `Connection: close` so the body reads
/// to EOF) with an optional `Origin`, and parse the response.
async fn http_get(addr: SocketAddr, path: &str, origin: Option<&str>) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
    let request = format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n{origin}\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let text = String::from_utf8_lossy(&raw).into_owned();

    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status code");

    HttpResponse {
        status,
        headers: head.to_lowercase(),
        body: body.to_string(),
    }
}

/// Stand up `serve` over a temp resource root holding a single pack file, and
/// return its address plus the temp-dir guard (kept alive for the test).
async fn serve_resources(file_rel: &str, contents: &str) -> (SocketAddr, TempDir) {
    let resources = TempDir::new("thingblock-link-serve").expect("temp resource dir");
    let file = resources.path().join(file_rel);
    fs::create_dir_all(file.parent().unwrap()).expect("create pack dir");
    fs::write(&file, contents).expect("write pack file");

    let resource_root = Arc::new(ResourceRoot::new(resources.path()).expect("resource root"));
    let daemon = Arc::new(Daemon::start(None).await.expect("daemon should start"));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        server::router::serve(listener, daemon, resource_root)
            .await
            .expect("serve");
    });

    (addr, resources)
}

#[tokio::test]
async fn serves_pack_file_with_js_mime_and_cors() {
    let (addr, _guard) = serve_resources(
        "extensions/peripheral/servo/manifest.js",
        "export default { id: 'servo' };",
    )
    .await;

    let resp = http_get(
        addr,
        "/resources/extensions/peripheral/servo/manifest.js",
        Some("https://editor.example"),
    )
    .await;

    assert_eq!(resp.status, 200, "pack file should be served");
    assert!(
        resp.body.contains("id: 'servo'"),
        "body should be the file contents, got: {}",
        resp.body
    );
    // ServeDir picks the MIME from the extension.
    assert!(
        resp.headers.contains("content-type: text/javascript"),
        "`.js` should be served as JavaScript; headers:\n{}",
        resp.headers
    );
    // CORS mirrors the cross-origin editor's origin so `import()` is allowed.
    assert!(
        resp.headers
            .contains("access-control-allow-origin: https://editor.example"),
        "CORS should reflect the request origin; headers:\n{}",
        resp.headers
    );
}

#[tokio::test]
async fn missing_pack_file_is_not_found() {
    let (addr, _guard) = serve_resources("extensions/peripheral/servo/manifest.js", "x").await;

    let resp = http_get(addr, "/resources/extensions/nope/manifest.js", None).await;
    assert_eq!(resp.status, 404, "an absent file under the root should 404");
}

#[tokio::test]
async fn traversal_outside_the_root_is_refused() {
    let (addr, _guard) = serve_resources("extensions/peripheral/servo/manifest.js", "x").await;

    // `..` segments must not escape the resource root onto the rest of the disk.
    let resp = http_get(addr, "/resources/../../Cargo.toml", None).await;
    assert_ne!(resp.status, 200, "traversal out of the root must not serve");
    assert!(
        !resp.body.contains("thingblock-link"),
        "must not leak repo files, got: {}",
        resp.body
    );
}

#[tokio::test]
async fn serves_pack_file_with_revalidating_cache_control() {
    let (addr, _guard) = serve_resources(
        "extensions/peripheral/thingbot-core/blocks.js",
        "export const registerBlocks = () => {};",
    )
    .await;

    let resp = http_get(
        addr,
        "/resources/extensions/peripheral/thingbot-core/blocks.js",
        None,
    )
    .await;

    assert_eq!(resp.status, 200, "pack file should be served");
    // With no directive a browser falls back to heuristic freshness — a tenth of the file's age by
    // `Last-Modified` — so a month-old pack file stays "fresh" for days and a deployed pack update
    // never reaches the editor, even across a restart.
    assert!(
        resp.headers.contains("cache-control: no-cache"),
        "pack files must be revalidated on every load; headers:\n{}",
        resp.headers
    );
}
