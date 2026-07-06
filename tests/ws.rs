//! End-to-end check of the WS pipe: accept → upgrade → parse envelope →
//! dispatch → serialize → `id` correlation, with a live daemon behind it. Drives
//! the real `listBoards` (M1) arm with an empty pnpid filter, which yields no
//! targets on a board-less host — exercising the whole round-trip through a live
//! `BoardList` RPC.

use std::net::Ipv4Addr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use thingblock_link::server;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::resource::ResourceRoot;
use thingblock_link::utils::tempdir::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn ws_round_trip_list_boards_returns_result() {
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

    socket
        .send(Message::Text(
            r#"{"id":"42","type":"listBoards","payload":{"pnpid":[]}}"#.into(),
        ))
        .await
        .expect("send request");

    let reply = loop {
        match socket
            .next()
            .await
            .expect("stream ended")
            .expect("ws message")
        {
            Message::Text(text) => break text,
            _ => continue,
        }
    };

    let json: serde_json::Value = serde_json::from_str(reply.as_str()).expect("parse reply");
    assert_eq!(json["id"], "42", "terminal reply must carry the request id");
    assert_eq!(json["type"], "result");
    // Empty pnpid filter matches nothing, so no targets on any host.
    assert_eq!(json["payload"]["targets"], serde_json::json!([]));
}
