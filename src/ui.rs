//! Native OS-facing UI: the system tray ([`tray`]) and the status window it
//! opens ([`window`]). Kept separate from the network-facing modules (`ws`,
//! `io`, `grpc`) — this is the only part of the crate that touches `tao`/`wry`.

pub mod tray;
pub mod window;
