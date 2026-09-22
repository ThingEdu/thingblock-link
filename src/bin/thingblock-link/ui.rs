//! Native OS-facing UI: the system tray and the status window it opens. The only part
//! of the helper that touches `tao`/`wry`, kept apart from the network-facing library.

pub mod tray;
pub mod window;
