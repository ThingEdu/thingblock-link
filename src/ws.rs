//! WebSocket server facing the browser/editor.
//!
//! [`protocol`] defines the `{id, type, payload}` envelope (the cross-repo
//! contract), [`server`] accepts connections, and [`session`] holds per-socket
//! state. [`batch`] coalesces streamed-text frames on the writer path. [`api`]
//! adds the one-shot HTTP JSON routes sharing the same listener.

pub mod api;
pub mod batch;
pub mod protocol;
pub mod server;
pub mod session;
