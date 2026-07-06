//! Standalone hardware bring-up harness for a full connect→subscribe→write
//! round trip through the `ble` module directly (no `/io` WS layer).
//!
//! Opens the first BLE adapter, scans for the ThingBot service UUID, connects
//! to the first hit, subscribes to its TX characteristic, writes an
//! ARE_YOU_THERE command to its RX characteristic, and prints notifications
//! for ~10 seconds. Run with real hardware and a running BlueZ/DBus session:
//!
//! ```sh
//! cargo run --example ble_session
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use thingblock_link::service::ble::transport::Ble;
use uuid::Uuid;

/// The ThingBot BLE service UUID devices advertise.
const THINGBOT_SERVICE_UUID: Uuid = Uuid::from_u128(0xaa700001_8f6a_4e2c_b369_4060e0bb33aa);
/// Commands go to the device on this characteristic.
const RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0xaa700002_8f6a_4e2c_b369_4060e0bb33aa);
/// The device reports back on this characteristic.
const TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0xaa700003_8f6a_4e2c_b369_4060e0bb33aa);
/// ThingBot's ARE_YOU_THERE command.
const ARE_YOU_THERE: [u8; 2] = [1, 6];

#[tokio::main]
async fn main() {
    let ble = Ble::discover()
        .await
        .expect("adapter subsystem error")
        .expect("no BLE adapter found on this machine");
    let ble = Arc::new(ble);

    println!("scanning for a ThingBot device...");
    let devices = ble
        .clone()
        .scan(vec![THINGBOT_SERVICE_UUID], None)
        .await
        .expect("failed to start scan");
    tokio::pin!(devices);
    let device = devices
        .next()
        .await
        .expect("no ThingBot device found before the stream ended");
    ble.stop_scan().await.expect("failed to stop scan");
    println!("found {device:?}, connecting...");

    let conn = ble.connect(&device.id).await.expect("failed to connect");

    let notifications = conn
        .notifications()
        .await
        .expect("failed to open notification stream");
    tokio::pin!(notifications);

    conn.subscribe(THINGBOT_SERVICE_UUID, TX_CHARACTERISTIC_UUID)
        .await
        .expect("failed to subscribe to TX characteristic");

    println!("writing ARE_YOU_THERE...");
    conn.write(
        THINGBOT_SERVICE_UUID,
        RX_CHARACTERISTIC_UUID,
        &ARE_YOU_THERE,
        true,
    )
    .await
    .expect("failed to write ARE_YOU_THERE");

    println!("listening for notifications for 10 seconds...");
    let deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = &mut deadline => break,
            notification = notifications.next() => match notification {
                Some(notification) => println!(
                    "notify {}: {:?}",
                    notification.characteristic, notification.data
                ),
                None => break,
            },
        }
    }

    conn.disconnect().await.expect("failed to disconnect");
}
