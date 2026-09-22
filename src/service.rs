//! Domain services the servers proxy to: the arduino-cli backend, the BLE `/io` channel,
//! and the served resource root.

pub mod arduino;
pub mod ble;
pub mod resource;
