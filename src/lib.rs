//! Shared core of the local tray helper (`thingblock-link`) and the public compile server
//! (`thingblock-link-cloud`): a translating proxy from the WS envelope to arduino-cli gRPC.

// Scaffolding: some modules are milestone stubs whose items aren't used yet. Remove this once
// every component is wired together and its items are actually used.
#![allow(dead_code)]

pub mod error;
pub mod server;
pub mod service;
pub mod utils;
