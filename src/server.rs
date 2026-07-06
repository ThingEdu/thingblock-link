//! HTTP + WebSocket server facing the browser/editor: the flash channel's WS
//! envelope and the one-shot HTTP JSON API share one listener.
//!
//! [`protocol`] defines the WS `{id, type, payload}` envelope (the cross-repo
//! contract), [`router`] builds the axum app and accepts connections, and
//! [`session`] holds per-socket WS state. [`batch`] coalesces streamed-text
//! frames on the WS writer path. [`api`] adds the one-shot HTTP JSON routes
//! sharing the same listener.

pub mod api;
pub mod batch;
pub mod protocol;
pub mod router;
pub mod session;
