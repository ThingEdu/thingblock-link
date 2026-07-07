//! End-to-end check of the HTTP platform-status API (`/api/platforms`) against
//! the in-repo config dir and data bundle. Offline-safe: `PlatformSearch` only
//! reads the local package indexes, which the bundle ships (including the esp32
//! index that backs the on-demand install). Raw-HTTP-over-TCP client in the
//! same spirit as `resource_serve.rs`.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use thingblock_link::server;
use thingblock_link::service::arduino::daemon::{Daemon, default_config_dir};
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

/// Stand up `serve` against the in-repo config dir (so platform results come
/// from the bundled indexes, not the host's `~/.arduino15`), and return its
/// address plus the temp-resource-dir guard.
async fn serve_api() -> (SocketAddr, TempDir) {
    let resources = TempDir::new("thingblock-link-api").expect("temp resource dir");
    let resource_root = Arc::new(ResourceRoot::new(resources.path()).expect("resource root"));
    let daemon = Arc::new(
        Daemon::start_with(None, Some(default_config_dir()))
            .await
            .expect("daemon should start against the shipped config"),
    );

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
async fn reports_bundled_platform_installed_with_cors() {
    let (addr, _guard) = serve_api().await;

    let resp = http_get(
        addr,
        "/api/platforms/arduino:avr",
        Some("https://editor.example"),
    )
    .await;

    assert_eq!(resp.status, 200, "bundled platform should be known");
    let platform: serde_json::Value = serde_json::from_str(&resp.body).expect("json body");
    assert_eq!(platform["id"], "arduino:avr");
    assert_eq!(
        platform["installed"], true,
        "arduino:avr ships in the bundle: {platform}"
    );
    // The editor fetches cross-origin, so the API needs the mirrored CORS origin.
    assert!(
        resp.headers
            .contains("access-control-allow-origin: https://editor.example"),
        "CORS should reflect the request origin; headers:\n{}",
        resp.headers
    );
}

#[tokio::test]
async fn unknown_platform_is_not_found() {
    let (addr, _guard) = serve_api().await;

    let resp = http_get(addr, "/api/platforms/nope:nope", None).await;

    assert_eq!(resp.status, 404);
    let body: serde_json::Value = serde_json::from_str(&resp.body).expect("json body");
    assert_eq!(body["code"], "invalidRequest");
    assert!(
        body["message"].as_str().unwrap_or("").contains("nope:nope"),
        "message should name the platform: {body}"
    );
}
