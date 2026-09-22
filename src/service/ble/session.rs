//! Per-connection state and the read→dispatch→write loop for one `/io` socket. No coalescing,
//! unlike the flash session: BLE frames are discrete, so each goes out as its own frame, in order.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::service::ble::protocol::{
    BleRequest, BleRequestBody, BleResponse, BleResponseBody, Responder,
};
use crate::service::ble::transport::{Ble, Conn};

/// Outbound responses queued toward the socket before backpressure; matches the flash channel.
const RESPONSE_CHANNEL_CAPACITY: usize = 64;

/// Pending peripheral-disconnect events before the relay backpressures; small, as they're rare.
const DISCONNECT_CHANNEL_CAPACITY: usize = 16;

/// State for one browser `/io` WS connection.
pub struct BleSession {
    /// The shared adapter, or `None` without a radio, in which case every BLE request fails.
    ble: Option<Arc<Ble>>,
    /// Live connections this session holds open, keyed by device id.
    connected: HashMap<String, Conn>,
    /// Each connected device's notification-pump task, keyed by device id.
    pumps: HashMap<String, JoinHandle<()>>,
    /// In-flight scans keyed by the starting request's id, so `cancel {id}` can stop the right one.
    scans: HashMap<String, CancellationToken>,
    /// Session-wide peripheral-disconnect relay, started lazily on the first `connect` and
    /// shared by every connection.
    disconnect_watch: Option<JoinHandle<()>>,
}

impl BleSession {
    pub fn new(ble: Option<Arc<Ble>>) -> Self {
        Self {
            ble,
            connected: HashMap::new(),
            pumps: HashMap::new(),
            scans: HashMap::new(),
            disconnect_watch: None,
        }
    }

    /// Drives the connection until the socket closes: a writer task sends queued responses while
    /// this loop dispatches requests and handles peripheral disconnects relayed from the watch.
    pub async fn run(mut self, socket: WebSocket) {
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::channel::<BleResponse>(RESPONSE_CHANNEL_CAPACITY);
        // The watch task only relays ids; this loop is the sole mutator of `connected`/`pumps`,
        // so it does the bookkeeping and framing.
        let (disconnect_tx, mut disconnect_rx) =
            mpsc::channel::<String>(DISCONNECT_CHANNEL_CAPACITY);

        let writer = tokio::spawn(async move {
            while let Some(response) = rx.recv().await {
                match serde_json::to_string(&response) {
                    Ok(json) => {
                        if sink.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        // A serialization failure is a bug in our response types, not the
                        // client's; log it and keep the socket usable.
                        warn!(error = %e, "failed to serialize io response");
                    }
                }
            }
        });

        loop {
            tokio::select! {
                message = stream.next() => {
                    let message = match message {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            warn!(error = %e, "io ws receive error; closing session");
                            break;
                        }
                        None => break,
                    };

                    match message {
                        Message::Text(text) => {
                            self.handle_text(text.as_str(), &tx, &disconnect_tx).await;
                        }
                        Message::Close(_) => break,
                        // The protocol has no binary/ping/pong messages; ignore them.
                        Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                    }
                }
                Some(device_id) = disconnect_rx.recv() => {
                    self.handle_peripheral_disconnect(device_id, &tx).await;
                }
            }
        }

        self.teardown().await;

        // Dropping `tx` ends the writer task, which closes the sink.
        drop(tx);
        let _ = writer.await;
    }

    /// Parses one text frame as an `/io` request and dispatches it; any failure is sent back
    /// as a terminal `error` for that request.
    async fn handle_text(
        &mut self,
        text: &str,
        tx: &mpsc::Sender<BleResponse>,
        disconnect_tx: &mpsc::Sender<String>,
    ) {
        let request: BleRequest = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(e) => {
                debug!(error = %e, "rejecting malformed io envelope");
                // No id to correlate against on a parse failure.
                let _ = tx
                    .send(BleResponse {
                        id: String::new(),
                        body: BleResponseBody::Error {
                            code: "invalidRequest".into(),
                            message: format!("malformed envelope: {e}"),
                        },
                    })
                    .await;
                return;
            }
        };

        let responder = Responder::new(request.id, tx.clone());
        if let Err(e) = self
            .dispatch(request.body, &responder, tx, disconnect_tx)
            .await
        {
            warn!(id = %responder.id(), code = e.code(), error = %e, "io request failed");
            responder
                .send(BleResponseBody::Error {
                    code: e.code().into(),
                    message: e.to_string(),
                })
                .await;
        }
    }

    /// Routes one request body to its handler. Every arm needs the adapter, so with none
    /// available every request fails uniformly.
    async fn dispatch(
        &mut self,
        body: BleRequestBody,
        responder: &Responder,
        tx: &mpsc::Sender<BleResponse>,
        disconnect_tx: &mpsc::Sender<String>,
    ) -> Result<()> {
        let ble = match &self.ble {
            Some(ble) => Arc::clone(ble),
            None => return Err(Error::Ble("no BLE adapter available".into())),
        };

        match body {
            BleRequestBody::Scan {
                services,
                name_prefix,
            } => {
                self.handle_scan(ble, services, name_prefix, responder)
                    .await
            }
            BleRequestBody::Cancel {} => {
                self.handle_cancel(responder);
                Ok(())
            }
            BleRequestBody::Connect { device_id } => {
                self.handle_connect(ble, device_id, responder, tx, disconnect_tx)
                    .await
            }
            BleRequestBody::Disconnect { device_id } => {
                self.handle_disconnect(device_id, responder).await
            }
            BleRequestBody::Write {
                device_id,
                service,
                characteristic,
                data,
                with_response,
            } => {
                self.handle_write(
                    device_id,
                    service,
                    characteristic,
                    data,
                    with_response,
                    responder,
                )
                .await
            }
            BleRequestBody::Read {
                device_id,
                service,
                characteristic,
            } => {
                self.handle_read(device_id, service, characteristic, responder)
                    .await
            }
            BleRequestBody::Subscribe {
                device_id,
                service,
                characteristic,
            } => {
                self.handle_subscribe(device_id, service, characteristic, responder)
                    .await
            }
            BleRequestBody::Unsubscribe {
                device_id,
                service,
                characteristic,
            } => {
                self.handle_unsubscribe(device_id, service, characteristic, responder)
                    .await
            }
        }
    }

    /// Starts a scan in a spawned task that streams hits and owns the terminal reply (`Result`
    /// once cancelled, `Error` if it never starts), so this returns once it's launched.
    async fn handle_scan(
        &mut self,
        ble: Arc<Ble>,
        services: Vec<String>,
        name_prefix: Option<String>,
        responder: &Responder,
    ) -> Result<()> {
        let services = services
            .iter()
            .map(|s| parse_uuid(s))
            .collect::<Result<Vec<_>>>()?;

        debug!(id = %responder.id(), ?services, ?name_prefix, "ble scan requested");

        let token = CancellationToken::new();
        self.scans.insert(responder.id().to_string(), token.clone());

        let responder = responder.clone();
        let scan_ble = Arc::clone(&ble);
        tokio::spawn(async move {
            let (generation, stream) = match scan_ble.clone().scan(services, name_prefix).await {
                Ok(scan) => scan,
                Err(e) => {
                    warn!(id = %responder.id(), error = %e, "ble scan failed to start");
                    responder
                        .send(BleResponseBody::Error {
                            code: e.code().into(),
                            message: e.to_string(),
                        })
                        .await;
                    return;
                }
            };
            debug!(id = %responder.id(), generation, "ble scan started");
            tokio::pin!(stream);

            loop {
                tokio::select! {
                    () = token.cancelled() => {
                        debug!(id = %responder.id(), generation, "ble scan cancelled");
                        let _ = scan_ble.release_scan(generation).await;
                        responder.send(BleResponseBody::Result(json!({}))).await;
                        return;
                    }
                    device = stream.next() => {
                        match device {
                            Some(device) => {
                                debug!(
                                    id = %responder.id(),
                                    device_id = %device.id,
                                    rssi = ?device.rssi,
                                    "ble scan hit"
                                );
                                responder
                                    .send(BleResponseBody::Device {
                                        device_id: device.id,
                                        name: device.name,
                                        rssi: device.rssi,
                                    })
                                    .await;
                            }
                            None => {
                                // The adapter stopped the scan itself; release our generation
                                // and still owe the client a terminal reply.
                                debug!(id = %responder.id(), generation, "ble scan ended by adapter");
                                let _ = scan_ble.release_scan(generation).await;
                                responder.send(BleResponseBody::Result(json!({}))).await;
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Cancels the scan whose request id this `cancel` targets; a no-op if it already ended.
    /// Removed here, not in the scan task, to keep `scans` mutation on the dispatch path.
    fn handle_cancel(&mut self, responder: &Responder) {
        debug!(id = %responder.id(), "ble cancel requested");
        if let Some(token) = self.scans.remove(responder.id()) {
            token.cancel();
        }
    }

    /// Connects to a peripheral and spawns its notification pump; the first successful connect
    /// also starts the session's disconnect watch.
    async fn handle_connect(
        &mut self,
        ble: Arc<Ble>,
        device_id: String,
        responder: &Responder,
        tx: &mpsc::Sender<BleResponse>,
        disconnect_tx: &mpsc::Sender<String>,
    ) -> Result<()> {
        debug!(id = %responder.id(), %device_id, "ble connect requested");
        let conn = ble.connect(&device_id).await?;
        let notifications = conn.notifications().await?;

        let notify_responder = Responder::new(String::new(), tx.clone());
        let notify_device_id = device_id.clone();
        let pump = tokio::spawn(async move {
            tokio::pin!(notifications);
            while let Some(notification) = notifications.next().await {
                debug!(
                    device_id = %notify_device_id,
                    characteristic = %notification.characteristic,
                    bytes = notification.data.len(),
                    "ble notification received"
                );
                notify_responder
                    .send(BleResponseBody::Notify {
                        device_id: notify_device_id.clone(),
                        characteristic: notification.characteristic,
                        data: BASE64.encode(notification.data),
                    })
                    .await;
            }
        });

        self.connected.insert(device_id.clone(), conn);
        if let Some(old_pump) = self.pumps.insert(device_id.clone(), pump) {
            // A reconnect under the same id orphaned the still-running old pump; stop it.
            old_pump.abort();
        }

        if self.disconnect_watch.is_none() {
            self.disconnect_watch = Some(spawn_disconnect_watch(ble, disconnect_tx.clone()));
        }

        debug!(id = %responder.id(), %device_id, "ble connect succeeded");
        responder
            .send(BleResponseBody::Result(json!({"deviceId": device_id})))
            .await;
        Ok(())
    }

    /// Disconnects a device this session holds open; safe for an unknown or already-gone id.
    async fn handle_disconnect(&mut self, device_id: String, responder: &Responder) -> Result<()> {
        debug!(id = %responder.id(), %device_id, "ble disconnect requested");
        if let Some(pump) = self.pumps.remove(&device_id) {
            pump.abort();
        }
        if let Some(conn) = self.connected.remove(&device_id) {
            conn.disconnect().await?;
        }
        responder.send(BleResponseBody::Result(json!({}))).await;
        Ok(())
    }

    async fn handle_write(
        &mut self,
        device_id: String,
        service: String,
        characteristic: String,
        data: String,
        with_response: bool,
        responder: &Responder,
    ) -> Result<()> {
        debug!(
            id = %responder.id(), %device_id, %service, %characteristic,
            bytes = data.len(), with_response,
            "ble write requested"
        );
        let conn = self.require_connected(&device_id)?;
        let service = parse_uuid(&service)?;
        let characteristic = parse_uuid(&characteristic)?;
        let bytes = BASE64
            .decode(data.as_bytes())
            .map_err(|e| Error::Ble(format!("invalid base64 write payload: {e}")))?;
        conn.write(service, characteristic, &bytes, with_response)
            .await?;
        responder.send(BleResponseBody::Result(json!({}))).await;
        Ok(())
    }

    async fn handle_read(
        &mut self,
        device_id: String,
        service: String,
        characteristic: String,
        responder: &Responder,
    ) -> Result<()> {
        debug!(id = %responder.id(), %device_id, %service, %characteristic, "ble read requested");
        let conn = self.require_connected(&device_id)?;
        let service = parse_uuid(&service)?;
        let characteristic = parse_uuid(&characteristic)?;
        let data = conn.read(service, characteristic).await?;
        debug!(id = %responder.id(), bytes = data.len(), "ble read completed");
        responder
            .send(BleResponseBody::Result(
                json!({"data": BASE64.encode(data)}),
            ))
            .await;
        Ok(())
    }

    async fn handle_subscribe(
        &mut self,
        device_id: String,
        service: String,
        characteristic: String,
        responder: &Responder,
    ) -> Result<()> {
        debug!(id = %responder.id(), %device_id, %service, %characteristic, "ble subscribe requested");
        let conn = self.require_connected(&device_id)?;
        let service = parse_uuid(&service)?;
        let characteristic = parse_uuid(&characteristic)?;
        conn.subscribe(service, characteristic).await?;
        responder.send(BleResponseBody::Result(json!({}))).await;
        Ok(())
    }

    async fn handle_unsubscribe(
        &mut self,
        device_id: String,
        service: String,
        characteristic: String,
        responder: &Responder,
    ) -> Result<()> {
        debug!(id = %responder.id(), %device_id, %service, %characteristic, "ble unsubscribe requested");
        let conn = self.require_connected(&device_id)?;
        let service = parse_uuid(&service)?;
        let characteristic = parse_uuid(&characteristic)?;
        conn.unsubscribe(service, characteristic).await?;
        responder.send(BleResponseBody::Result(json!({}))).await;
        Ok(())
    }

    fn require_connected(&self, device_id: &str) -> Result<&Conn> {
        self.connected
            .get(device_id)
            .ok_or_else(|| Error::Ble(format!("device {device_id} is not connected")))
    }

    /// Handles a relayed disconnect: drops the device's bookkeeping and tells the browser. Events
    /// are adapter-wide, so ids this session never connected are ignored.
    async fn handle_peripheral_disconnect(
        &mut self,
        device_id: String,
        tx: &mpsc::Sender<BleResponse>,
    ) {
        if self.connected.remove(&device_id).is_none() {
            return;
        }
        debug!(%device_id, "ble peripheral disconnected unexpectedly");
        if let Some(pump) = self.pumps.remove(&device_id) {
            pump.abort();
        }

        let response = BleResponse {
            id: String::new(),
            body: BleResponseBody::Disconnected { device_id },
        };
        if tx.send(response).await.is_err() {
            warn!("io disconnect notice dropped: ws writer closed");
        }
    }

    /// Stops every background task and live connection before the socket closes for good.
    async fn teardown(&mut self) {
        if let Some(watch) = self.disconnect_watch.take() {
            watch.abort();
        }
        for (_, pump) in self.pumps.drain() {
            pump.abort();
        }
        // Cancelling is enough: each scan task releases its own generation, and stopping the
        // adapter here could kill a newer scan owned by another session.
        for (_, token) in self.scans.drain() {
            token.cancel();
        }
        for (_, conn) in self.connected.drain() {
            let _ = conn.disconnect().await;
        }
    }
}

/// Parses a wire UUID, mapping a bad value onto the same `Error::Ble` as other BLE failures.
fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| Error::Ble(format!("invalid uuid {s}: {e}")))
}

/// Spawns the task relaying adapter peripheral disconnects into `run`'s loop. The event stream
/// is adapter-wide, so one watch per session covers every device it connects.
fn spawn_disconnect_watch(ble: Arc<Ble>, disconnect_tx: mpsc::Sender<String>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let events = match ble.disconnect_events().await {
            Ok(events) => events,
            Err(e) => {
                warn!(error = %e, "failed to start ble disconnect watch");
                return;
            }
        };
        tokio::pin!(events);
        while let Some(device_id) = events.next().await {
            if disconnect_tx.send(device_id).await.is_err() {
                break;
            }
        }
    })
}
