//! thingblock-link — the local native helper that backs the thingblock-editor
//! firmware module's local-helper compile/flash mode.
//!
//! Two faces: an HTTP/WebSocket server for the browser/editor, and a gRPC client
//! to the `arduino-cli` daemon. It is a translating proxy — the browser speaks
//! our minimal `{id, type, payload}` envelope ([`server::protocol`]) and the
//! bridge turns each request into an `ArduinoCoreService` streaming call.
//!
//! The components live here (rather than in the binary) so integration tests in
//! `tests/` and the thin `main.rs` binary share one module tree. See
//! `.agents/docs/21-06_01.arduino-helper-design.md` for the protocol and the
//! milestone roadmap.

// Scaffolding: modules are stubs filled in per-milestone. Remove this once the
// components are wired together and their items are actually used.
#![allow(dead_code)]

pub mod error;
pub mod server;
pub mod service;
pub mod ui;
pub mod utils;
