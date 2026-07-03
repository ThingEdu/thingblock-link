//! End-to-end checks of the WS `installPlatform` arm against the in-repo config
//! dir. Offline-safe: every case ends in a terminal `error` before any download
//! starts (a malformed id, an unknown platform, a held install lock). The happy
//! path pulls hundreds of MB over the network, so it is covered by manual
//! verification, not CI.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use thingblock_link::daemon::{Daemon, default_config_dir};
use thingblock_link::resource::ResourceRoot;
use thingblock_link::utils::tempdir::TempDir;
use thingblock_link::ws;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Stand up `serve` against the in-repo config dir and return the address, the
/// daemon handle (for lock manipulation), and the temp-resource-dir guard.
async fn serve_ws() -> (SocketAddr, Arc<Daemon>, TempDir) {
    let daemon = Arc::new(
        Daemon::start_with(None, Some(default_config_dir()))
            .await
            .expect("daemon should start against the shipped config"),
    );
    let resources = TempDir::new("thingblock-link-install").expect("temp resource dir");
    let resource_root = Arc::new(ResourceRoot::new(resources.path()).expect("resource root"));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ws listener");
    let addr = listener.local_addr().expect("listener address");
    let serve_daemon = daemon.clone();
    tokio::spawn(async move {
        ws::server::serve(listener, serve_daemon, resource_root)
            .await
            .expect("serve");
    });

    (addr, daemon, resources)
}

/// Send one request and read frames until the terminal (`result`/`error`) for
/// its id arrives.
async fn request_terminal(addr: SocketAddr, request: &str, id: &str) -> serde_json::Value {
    let url = format!("ws://{addr}/");
    let (mut socket, _resp) = connect_async(url.as_str()).await.expect("ws connect");
    socket
        .send(Message::Text(request.to_string().into()))
        .await
        .expect("send request");

    loop {
        let Message::Text(text) = socket
            .next()
            .await
            .expect("stream ended")
            .expect("ws message")
        else {
            continue;
        };
        let json: serde_json::Value = serde_json::from_str(text.as_str()).expect("parse reply");
        if json["id"] == id && (json["type"] == "result" || json["type"] == "error") {
            return json;
        }
    }
}

#[tokio::test]
async fn malformed_platform_id_is_invalid_request() {
    let (addr, _daemon, _guard) = serve_ws().await;

    let reply = request_terminal(
        addr,
        r#"{"id":"1","type":"installPlatform","payload":{"platform":"noarch"}}"#,
        "1",
    )
    .await;

    assert_eq!(reply["type"], "error");
    assert_eq!(reply["payload"]["code"], "invalidRequest");
    assert!(
        reply["payload"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("vendor:architecture"),
        "message should explain the expected shape: {reply}"
    );
}

#[tokio::test]
async fn unknown_platform_fails_and_releases_the_install_lock() {
    let (addr, daemon, _guard) = serve_ws().await;

    let reply = request_terminal(
        addr,
        r#"{"id":"2","type":"installPlatform","payload":{"platform":"nope:nope"}}"#,
        "2",
    )
    .await;

    assert_eq!(
        reply["type"], "error",
        "an unindexed platform cannot install: {reply}"
    );

    // The spawned task holds the daemon-wide install lock; a failed install
    // must release it or every later install would report busy. The release
    // races the terminal reply, so poll briefly.
    let mut released = false;
    for _ in 0..50 {
        if daemon.install_lock().try_lock_owned().is_ok() {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        released,
        "install lock should release after a failed install"
    );
}

#[tokio::test]
async fn concurrent_install_is_rejected_as_busy() {
    let (addr, daemon, _guard) = serve_ws().await;

    // Deterministically simulate an in-flight install by holding the lock.
    let _guard_lock = daemon
        .install_lock()
        .try_lock_owned()
        .expect("lock should be free");

    let reply = request_terminal(
        addr,
        r#"{"id":"3","type":"installPlatform","payload":{"platform":"esp32:esp32"}}"#,
        "3",
    )
    .await;

    assert_eq!(reply["type"], "error");
    assert_eq!(reply["payload"]["code"], "invalidRequest");
    assert!(
        reply["payload"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("already running"),
        "message should say an install is in flight: {reply}"
    );
}
