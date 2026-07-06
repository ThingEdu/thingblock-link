//! The `/io` WebSocket channel: BLE access for the editor's device-facing
//! extensions, kept entirely separate from the arduino-cli "flash" channel
//! ([`crate::server`] / [`crate::service::arduino`]). [`BleSession`] parses each
//! request and dispatches straight to [`transport`] — scan, connect,
//! read/write/subscribe, and disconnect — with no translation layer.
//!
//! Isolation is deliberate, not incidental: this module must never import
//! from the flash channel, so the two can evolve (or be torn out) independently.

pub mod protocol;
pub mod session;
pub mod transport;

use std::sync::Arc;

use axum::extract::WebSocketUpgrade;
use axum::extract::ws::WebSocket;
use axum::response::Response;

use crate::service::ble::session::BleSession;
use crate::service::ble::transport::Ble;

/// Upgrade an HTTP connection to WebSocket and hand the socket to a fresh
/// [`BleSession`]. `ble` is `None` on a machine with no adapter (or one whose
/// discovery failed); the session still accepts connections, it just fails
/// every request that would need BLE.
pub async fn upgrade(ws: WebSocketUpgrade, ble: Option<Arc<Ble>>) -> Response {
    ws.on_upgrade(move |socket: WebSocket| BleSession::new(ble).run(socket))
}
