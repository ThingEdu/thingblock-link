//! Standalone hardware bring-up harness for the `ble` module.
//!
//! Opens the first BLE adapter, scans for the ThingBot service UUID for ~10
//! seconds, and prints every discovered device. Run with real hardware and a
//! running BlueZ/DBus session:
//!
//! ```sh
//! cargo run --example ble_scan
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use thingblock_link::service::ble::transport::Ble;
use uuid::Uuid;

/// The ThingBot BLE service UUID devices advertise.
const THINGBOT_SERVICE_UUID: Uuid = Uuid::from_u128(0xaa700001_8f6a_4e2c_b369_4060e0bb33aa);

#[tokio::main]
async fn main() {
    let ble = Ble::discover()
        .await
        .expect("adapter subsystem error")
        .expect("no BLE adapter found on this machine");

    let (_generation, devices) = Arc::new(ble)
        .scan(vec![THINGBOT_SERVICE_UUID], None)
        .await
        .expect("failed to start scan");
    tokio::pin!(devices);

    println!("scanning for ThingBot devices for 10 seconds...");
    let deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = &mut deadline => break,
            device = devices.next() => match device {
                Some(device) => println!("{device:?}"),
                None => break,
            },
        }
    }
}
