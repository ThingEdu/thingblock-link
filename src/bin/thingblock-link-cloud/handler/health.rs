//! `GET /health` controller.

/// Liveness only — probing the daemon would take an instance the next compile
/// wants, and a dead daemon already surfaces per-request as `daemon`.
pub async fn health() -> &'static str {
    "ok"
}
