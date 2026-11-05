//! Core of the local tray helper (`thingblock-link`): a translating proxy from the WS envelope
//! to arduino-cli gRPC.

// Scaffolding: some modules are milestone stubs whose items aren't used yet. Remove this once
// every component is wired together and its items are actually used.
#![allow(dead_code)]

pub mod error;
pub mod utils;

pub mod gateway;
pub mod handler;
