//! `/io` BLE channel; must never import from the flash channel so either can be torn out alone.

pub mod protocol;
pub mod session;
pub mod transport;

use std::sync::Arc;

use axum::extract::WebSocketUpgrade;
use axum::extract::ws::WebSocket;
use axum::response::Response;

use crate::service::ble::session::BleSession;
use crate::service::ble::transport::Ble;

/// `ble` is `None` without a usable adapter; the session still runs but fails every BLE request.
pub async fn upgrade(ws: WebSocketUpgrade, ble: Option<Arc<Ble>>) -> Response {
    ws.on_upgrade(move |socket: WebSocket| BleSession::new(ble).run(socket))
}
