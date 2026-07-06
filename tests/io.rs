//! End-to-end check of the `/io` channel in isolation: no hardware, no daemon,
//! no flash-channel server. Builds a minimal axum app with just the `/io`
//! route (`ble = None`) and drives it with a real WS client, proving the
//! route + envelope round-trip works. `ble = None` simulates a machine with
//! no Bluetooth radio, so every dispatch arm fails uniformly without needing
//! real hardware.

use std::net::Ipv4Addr;

use axum::Router;
use axum::extract::WebSocketUpgrade;
use axum::routing::any;
use futures::{SinkExt, StreamExt};
use thingblock_link::service::ble;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn io_scan_without_adapter_returns_ble_error() {
    let app = Router::new().route(
        "/io",
        any(move |ws: WebSocketUpgrade| ble::upgrade(ws, None)),
    );

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind io listener");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve io app");
    });

    let url = format!("ws://{addr}/io");
    let (mut socket, _resp) = connect_async(url.as_str()).await.expect("ws connect");

    socket
        .send(Message::Text(
            r#"{"id":"1","type":"scan","payload":{"services":[]}}"#.into(),
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
    assert_eq!(json["id"], "1", "terminal reply must carry the request id");
    assert_eq!(json["type"], "error");
    assert_eq!(json["payload"]["code"], "ble");
    assert!(
        json["payload"]["message"]
            .as_str()
            .expect("message is a string")
            .contains("no BLE adapter"),
        "error message should explain the missing adapter: {json}"
    );
}

/// A `connect` request fails the same way as `scan` with no adapter: every
/// dispatch arm checks `ble` up front before doing anything device-specific.
#[tokio::test]
async fn io_connect_without_adapter_returns_ble_error() {
    let app = Router::new().route(
        "/io",
        any(move |ws: WebSocketUpgrade| ble::upgrade(ws, None)),
    );

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind io listener");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve io app");
    });

    let url = format!("ws://{addr}/io");
    let (mut socket, _resp) = connect_async(url.as_str()).await.expect("ws connect");

    socket
        .send(Message::Text(
            r#"{"id":"1","type":"connect","payload":{"deviceId":"anything"}}"#.into(),
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
    assert_eq!(json["id"], "1", "terminal reply must carry the request id");
    assert_eq!(json["type"], "error");
    assert_eq!(json["payload"]["code"], "ble");
}
