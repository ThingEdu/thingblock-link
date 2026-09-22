// Liveness only: probing the daemon would steal an instance a compile wants.
pub async fn health() -> &'static str {
    "ok"
}
