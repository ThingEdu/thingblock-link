//! End-to-end check of the `connect` / `disconnect` session arms over the WS
//! pipe. These are helper-side state only — the daemon is connectionless
//! per-port — so a live daemon is spun up just to satisfy the session/server;
//! the requests themselves never reach it.

use std::net::Ipv4Addr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use thingblock_link::server;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::resource::ResourceRoot;
use thingblock_link::utils::tempdir::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Send one envelope and return the next text reply parsed as JSON.
async fn round_trip(socket: &mut Socket, request: &str) -> serde_json::Value {
    socket
        .send(Message::Text(request.into()))
        .await
        .expect("send request");
    loop {
        match socket
            .next()
            .await
            .expect("stream ended")
            .expect("ws message")
        {
            Message::Text(text) => {
                return serde_json::from_str(text.as_str()).expect("parse reply");
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn connect_disconnect_round_trip() {
    let daemon = Arc::new(Daemon::start(None).await.expect("daemon should start"));
    let resources = TempDir::new("thingblock-link-test-res").expect("temp resource dir");
    let resource_root = Arc::new(ResourceRoot::new(resources.path()).expect("resource root"));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ws listener");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        server::router::serve(listener, daemon, resource_root)
            .await
            .expect("serve");
    });

    let url = format!("ws://{addr}/");
    let (mut socket, _resp) = connect_async(url.as_str()).await.expect("ws connect");

    // connect stores the selected port and replies with an empty result.
    let connected = round_trip(
        &mut socket,
        r#"{"id":"1","type":"connect","payload":{"port":"/dev/ttyUSB0"}}"#,
    )
    .await;
    assert_eq!(connected["id"], "1");
    assert_eq!(connected["type"], "result");
    assert_eq!(connected["payload"], serde_json::json!({}));

    // disconnect clears it and replies with an empty result.
    let disconnected = round_trip(
        &mut socket,
        r#"{"id":"2","type":"disconnect","payload":{}}"#,
    )
    .await;
    assert_eq!(disconnected["id"], "2");
    assert_eq!(disconnected["type"], "result");
    assert_eq!(disconnected["payload"], serde_json::json!({}));

    // An empty port is rejected at the boundary.
    let rejected = round_trip(
        &mut socket,
        r#"{"id":"3","type":"connect","payload":{"port":""}}"#,
    )
    .await;
    assert_eq!(rejected["id"], "3");
    assert_eq!(rejected["type"], "error");
    assert_eq!(rejected["payload"]["code"], "invalidRequest");
}
