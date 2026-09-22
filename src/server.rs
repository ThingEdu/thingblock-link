//! HTTP + WebSocket server facing the browser/editor: the flash channel's WS envelope and the
//! one-shot HTTP JSON API share one listener.

pub mod api;
pub mod batch;
pub mod protocol;
pub mod router;
pub mod session;
