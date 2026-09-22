//! The `/io` WebSocket channel: BLE access for the editor's device extensions. Never imports
//! from the arduino-cli flash channel, so either can evolve or be torn out independently.

pub mod protocol;
pub mod session;
pub mod transport;

use std::sync::Arc;

use axum::extract::WebSocketUpgrade;
use axum::extract::ws::WebSocket;
use axum::response::Response;

use crate::service::ble::session::BleSession;
use crate::service::ble::transport::Ble;

/// Upgrades an HTTP connection and hands the socket to a fresh `BleSession`. `ble` is `None`
/// without a usable adapter; the session still runs but fails every request needing BLE.
pub async fn upgrade(ws: WebSocketUpgrade, ble: Option<Arc<Ble>>) -> Response {
    ws.on_upgrade(move |socket: WebSocket| BleSession::new(ble).run(socket))
}
