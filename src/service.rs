//! Domain services the crate proxies to: the arduino-cli backend
//! ([`arduino`]), the BLE transport + `/io` channel ([`ble`]), and the served
//! resource root ([`resource`]).

pub mod arduino;
pub mod ble;
pub mod resource;
