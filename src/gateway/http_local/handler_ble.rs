//! The `/ws/ble` WebSocket session: reads `{id, type, payload}` requests from the editor's device
//! extensions, runs them against the BLE service, and pushes scan hits, notifications and
//! disconnects back.

use std::collections::HashMap;
use std::pin::pin;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::gateway::GatewayContext;
use crate::gateway::http_local::model_ble::{
    BleRequest, BleRequestBody, BleResponse, BleResponseBody,
};
use crate::service::ble::{BleService, Conn};

/// How many responses may queue toward the socket before backpressure applies.
const RESPONSE_CHANNEL_CAPACITY: usize = 64;

/// How many peripheral disconnects may queue toward the session loop; they are rare.
const DISCONNECT_CHANNEL_CAPACITY: usize = 16;

/// `GET /ws/ble`: upgrades to WebSocket and runs one [`BleSession`] on the socket.
pub async fn ws(ws: WebSocketUpgrade, State(context): State<GatewayContext>) -> Response {
    ws.on_upgrade(move |socket| BleSession::new(context.ble).run(socket))
}

/// A device this session connected, with the task pumping its notifications out.
struct Device {
    conn: Conn,
    pump: JoinHandle<()>,
}

/// State of one editor BLE connection.
struct BleSession {
    /// `None` without a BLE radio; every request then fails.
    ble: Option<Arc<BleService>>,
    devices: HashMap<String, Device>,
    /// Cancellation tokens of running scans by request id.
    scans: HashMap<String, CancellationToken>,
    /// Relays adapter-wide peripheral disconnects into the session loop, started on the first
    /// `connect`.
    disconnect_watch: Option<JoinHandle<()>>,
}

/// Sends one request's responses to the writer, stamped with its `id`.
#[derive(Clone)]
struct Responder {
    id: String,
    tx: mpsc::Sender<BleResponse>,
}

impl Responder {
    /// A closed channel means the browser is gone, so the response is logged and dropped.
    async fn send(&self, body: BleResponseBody) {
        let response = BleResponse {
            id: self.id.clone(),
            body,
        };
        if self.tx.send(response).await.is_err() {
            warn!(id = %self.id, "ble response dropped: writer closed");
        }
    }

    /// Sends a terminal `error` with one of the three WS codes.
    async fn send_error(&self, error: &Error) {
        let code = match error {
            Error::Cancelled => "cancelled",
            Error::Gateway(_) => "invalidRequest",
            _ => "failed",
        };
        warn!(id = %self.id, code, %error, "ble request failed");
        self.send(BleResponseBody::Error {
            code: code.into(),
            message: error.to_string(),
        })
        .await;
    }
}

impl BleSession {
    fn new(ble: Option<Arc<BleService>>) -> Self {
        Self {
            ble,
            devices: HashMap::new(),
            scans: HashMap::new(),
            disconnect_watch: None,
        }
    }

    /// Reads and dispatches requests, and handles relayed peripheral disconnects, until the
    /// socket closes; then tears the session down.
    async fn run(mut self, socket: WebSocket) {
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::channel::<BleResponse>(RESPONSE_CHANNEL_CAPACITY);
        let (disconnect_tx, mut disconnect_rx) = mpsc::channel(DISCONNECT_CHANNEL_CAPACITY);

        let writer = tokio::spawn(async move {
            while let Some(response) = rx.recv().await {
                let json = serde_json::to_string(&response).expect("BleResponse always serializes");
                if sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        });

        loop {
            tokio::select! {
                message = stream.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        self.handle_text(text.as_str(), &tx, &disconnect_tx).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        warn!(error = %e, "ble ws receive error; closing session");
                        break;
                    }
                },
                Some(device_id) = disconnect_rx.recv() => {
                    self.peripheral_disconnected(device_id, &tx).await;
                }
            }
        }

        self.teardown().await;
        // The writer ends once this and every task's sender are dropped.
        drop(tx);
        let _ = writer.await;
    }

    /// Parses one frame and dispatches it; a failed dispatch is the request's terminal `error`.
    async fn handle_text(
        &mut self,
        text: &str,
        tx: &mpsc::Sender<BleResponse>,
        disconnect_tx: &mpsc::Sender<String>,
    ) {
        let request: BleRequest = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(e) => {
                // A malformed envelope has no id to correlate the error with.
                let responder = Responder {
                    id: String::new(),
                    tx: tx.clone(),
                };
                responder
                    .send_error(&Error::Gateway(format!("malformed envelope: {e}")))
                    .await;
                return;
            }
        };

        let responder = Responder {
            id: request.id,
            tx: tx.clone(),
        };
        if let Err(e) = self
            .dispatch(request.body, &responder, tx, disconnect_tx)
            .await
        {
            responder.send_error(&e).await;
        }
    }

    /// Runs one request. Each request ends with exactly one terminal response: either this
    /// returns `Err`, or it (or the scan task it spawns) sends the `result`. `cancel` has no
    /// reply of its own.
    async fn dispatch(
        &mut self,
        body: BleRequestBody,
        responder: &Responder,
        tx: &mpsc::Sender<BleResponse>,
        disconnect_tx: &mpsc::Sender<String>,
    ) -> Result<()> {
        let Some(ble) = self.ble.clone() else {
            return Err(Error::Ble("no BLE adapter available".into()));
        };

        match body {
            BleRequestBody::Scan {
                services,
                name_prefix,
            } => self.scan(ble, services, name_prefix, responder).await?,
            BleRequestBody::Connect { device_id } => {
                self.connect(ble, device_id, responder, tx, disconnect_tx)
                    .await?;
            }
            BleRequestBody::Disconnect { device_id } => {
                if let Some(device) = self.devices.remove(&device_id) {
                    device.pump.abort();
                    device.conn.disconnect().await?;
                }
                responder.send(BleResponseBody::Ok {}).await;
            }
            BleRequestBody::Write {
                device_id,
                service,
                characteristic,
                data,
                with_response,
            } => {
                let bytes = BASE64
                    .decode(data.as_bytes())
                    .map_err(|e| Error::Gateway(format!("write data is not base64: {e}")))?;
                self.conn(&device_id)?
                    .write(service, characteristic, &bytes, with_response)
                    .await?;
                responder.send(BleResponseBody::Ok {}).await;
            }
            BleRequestBody::Read {
                device_id,
                service,
                characteristic,
            } => {
                let data = self.conn(&device_id)?.read(service, characteristic).await?;
                responder
                    .send(BleResponseBody::Read {
                        data: BASE64.encode(data),
                    })
                    .await;
            }
            BleRequestBody::Subscribe {
                device_id,
                service,
                characteristic,
            } => {
                self.conn(&device_id)?
                    .subscribe(service, characteristic)
                    .await?;
                responder.send(BleResponseBody::Ok {}).await;
            }
            BleRequestBody::Unsubscribe {
                device_id,
                service,
                characteristic,
            } => {
                self.conn(&device_id)?
                    .unsubscribe(service, characteristic)
                    .await?;
                responder.send(BleResponseBody::Ok {}).await;
            }
            // The envelope `id` is the scan's id; its task sends the `result {}`.
            BleRequestBody::Cancel {} => match self.scans.remove(&responder.id) {
                Some(token) => token.cancel(),
                None => debug!(id = responder.id, "cancel: no running scan"),
            },
        }

        Ok(())
    }

    /// Starts a scan and spawns the task streaming its hits as `device`. The task ends with
    /// `result {}` once the scan is cancelled or the adapter stops it.
    async fn scan(
        &mut self,
        ble: Arc<BleService>,
        services: Vec<Uuid>,
        name_prefix: Option<String>,
        responder: &Responder,
    ) -> Result<()> {
        let (generation, hits) = ble.clone().scan(services, name_prefix).await?;
        let token = CancellationToken::new();
        self.scans.insert(responder.id.clone(), token.clone());
        debug!(id = responder.id, generation, "ble scan started");

        let responder = responder.clone();
        tokio::spawn(async move {
            let mut hits = pin!(hits);
            loop {
                tokio::select! {
                    () = token.cancelled() => break,
                    hit = hits.next() => match hit {
                        Some(hit) => {
                            responder
                                .send(BleResponseBody::Device {
                                    device_id: hit.id,
                                    name: hit.name,
                                    rssi: hit.rssi,
                                })
                                .await;
                        }
                        None => break,
                    },
                }
            }
            if let Err(e) = ble.release_scan(generation).await {
                warn!(id = responder.id, error = %e, "ble scan release failed");
            }
            responder.send(BleResponseBody::Ok {}).await;
        });
        Ok(())
    }

    /// Connects a scanned device and spawns the pump sending its notifications as `notify`.
    async fn connect(
        &mut self,
        ble: Arc<BleService>,
        device_id: String,
        responder: &Responder,
        tx: &mpsc::Sender<BleResponse>,
        disconnect_tx: &mpsc::Sender<String>,
    ) -> Result<()> {
        if self.devices.contains_key(&device_id) {
            return Err(Error::Gateway(format!(
                "device {device_id} is already connected"
            )));
        }
        // Started before connecting so a drop right after the connect isn't missed.
        if self.disconnect_watch.is_none() {
            self.disconnect_watch =
                Some(spawn_disconnect_watch(ble.clone(), disconnect_tx.clone()));
        }

        let conn = ble.connect(&device_id).await?;
        let notifications = conn.notifications().await?;
        let notify = Responder {
            id: String::new(),
            tx: tx.clone(),
        };
        let pump_device_id = device_id.clone();
        let pump = tokio::spawn(async move {
            let mut notifications = pin!(notifications);
            while let Some(notification) = notifications.next().await {
                notify
                    .send(BleResponseBody::Notify {
                        device_id: pump_device_id.clone(),
                        characteristic: notification.characteristic,
                        data: BASE64.encode(notification.data),
                    })
                    .await;
            }
        });

        debug!(id = responder.id, %device_id, "ble connected");
        self.devices
            .insert(device_id.clone(), Device { conn, pump });
        responder.send(BleResponseBody::Connect { device_id }).await;
        Ok(())
    }

    /// Drops a device that disconnected on its own and tells the editor. Events are
    /// adapter-wide, so devices this session doesn't hold are ignored.
    async fn peripheral_disconnected(&mut self, device_id: String, tx: &mpsc::Sender<BleResponse>) {
        let Some(device) = self.devices.remove(&device_id) else {
            return;
        };
        device.pump.abort();
        debug!(%device_id, "ble device disconnected");
        let notice = Responder {
            id: String::new(),
            tx: tx.clone(),
        };
        notice
            .send(BleResponseBody::Disconnected { device_id })
            .await;
    }

    fn conn(&self, device_id: &str) -> Result<&Conn> {
        self.devices
            .get(device_id)
            .map(|device| &device.conn)
            .ok_or_else(|| Error::Gateway(format!("device {device_id} is not connected")))
    }

    /// Stops every scan, pump and connection this session holds.
    async fn teardown(&mut self) {
        if let Some(watch) = self.disconnect_watch.take() {
            watch.abort();
        }
        // Each scan task releases its own scan; stopping the adapter here could kill a newer
        // scan owned by another session.
        for (_, token) in self.scans.drain() {
            token.cancel();
        }
        for (device_id, device) in self.devices.drain() {
            device.pump.abort();
            if let Err(e) = device.conn.disconnect().await {
                warn!(%device_id, error = %e, "ble disconnect on teardown failed");
            }
        }
    }
}

/// Spawns the task relaying adapter peripheral disconnects into the session loop, which owns
/// the device bookkeeping.
fn spawn_disconnect_watch(
    ble: Arc<BleService>,
    disconnect_tx: mpsc::Sender<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let events = match ble.disconnect_events().await {
            Ok(events) => events,
            Err(e) => {
                warn!(error = %e, "ble disconnect watch failed to start");
                return;
            }
        };
        let mut events = pin!(events);
        while let Some(device_id) = events.next().await {
            if disconnect_tx.send(device_id).await.is_err() {
                break;
            }
        }
    })
}
